// SPDX-License-Identifier: CC0-1.0

//! RUNNING the application a step submitted: where its code comes from, what runs it, and
//! what the step it belongs to ends in.
//!
//! "Amazon EMR executes each step in the order listed. For a step to be considered complete,
//! the main function must exit with a zero exit code and all Hadoop jobs started while the
//! step was running must have completed and run successfully." (`AddJobFlowSteps` doc.) So
//! the state a step ends in IS the outcome of running what it submitted, and for the one
//! class of submission this emulator can run — a `spark-submit` of a Python application in
//! Amazon S3 ([`super::spark_application`]) — that outcome is produced here, by really
//! running it:
//!
//! * the application's code is READ from the emulator's own Amazon S3, over the framework's
//!   in-process callback ([`smithy_server_runtime::OperationContext::callback`]) — the same
//!   transport a client's own S3 request takes, so the bytes that run are the bytes that were
//!   uploaded;
//! * it is EXECUTED by the contained Python interpreter this workspace already embeds (the
//!   Lambda service crate's `state::execution::python`), with the Spark runtime this crate
//!   supplies ([`super::spark_runtime`]) and the AWS SDK modules that interpreter puts on a
//!   guest's import path — so the application's own transformations are computed and its own
//!   writes are really made, into this emulator's object store, signed as the role the step
//!   runs as;
//! * what the STEP reports is what that run answered: an application that ran to its end
//!   COMPLETED, one that raised FAILED carrying its own exception in `FailureDetails`, and
//!   one stopped by a bound of this emulator FAILED saying which bound.
//!
//! # When it runs
//!
//! A step's application is owed from the instant its cluster STARTED the step — the queue,
//! the concurrency slots and the timeline are the cluster's own runner's
//! ([`ClusterRecord::advance_to`]) — and it is STARTED by the next operation that reports that
//! step or its cluster, with no store guard held, exactly as a Glue job run's script is
//! started by the next call that reads the run. The call that starts one waits a moment for it
//! ([`APPLICATION_FIRST_WAIT`]) and no longer: an application is caller-supplied work of
//! whatever size, so a run that has not answered goes on without the call that started it and
//! a later call collects its answer ([`PendingApplicationRun`]) — which is also what makes one
//! application run ONCE however often its step is read. A step whose application has not
//! answered does not END ([`StepRecord::awaits_application`]): the state it ends in is the one
//! its application answered, so the step stays `RUNNING` until there is an answer, and no step
//! is ever reported COMPLETED for work nothing performed.
//!
//! # Bounds
//!
//! An application is caller-supplied code, so every bound is this emulator's and none of them
//! is reachable from the application: how many applications this process runs at once
//! ([`MAX_CONCURRENT_APPLICATION_RUNS`], with a bounded wait for a permit), how many ONE call
//! starts ([`MAX_APPLICATION_RUNS_PER_CALL`]), how large the application may be
//! ([`MAX_APPLICATION_BYTES`], enforced by the ranged read), the span of the RUN's own time it
//! may last and the step, memory, recursion and output ceilings the interpreter is given
//! ([`application_budget`]), and the HOST time one run may take beside them
//! ([`APPLICATION_HOST_CEILING`]). The containment — the native modules that are never
//! registered, the host surface that is compiled out and the stop no guest call can clear — is
//! that engine's, which is why nothing of it is stood up again here.

use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::python;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    function_sdk_access, AwsAccess,
};
use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

use super::*;

/// How large an application file this emulator reads.
///
/// CHOSEN, not an AWS limit: the file is an object a caller uploaded, so what one run takes
/// into this process is bounded here rather than by the object's own size. The bound is
/// enforced by the RANGED read, so nothing larger is ever buffered.
pub const MAX_APPLICATION_BYTES: usize = 1024 * 1024;

/// The longest ONE run of a submitted application may last, in the RUN's own time.
///
/// CHOSEN, not an AWS duration: a real Spark application runs for as long as its cluster gives
/// it, so this is the span an application gets here, and one that runs past it fails the step
/// on that bound rather than being left running.
///
/// It is a span of the RUN's own time rather than of this host's: it is spent by the waits the
/// application asks for and by one tick per traced event it delivers
/// ([`APPLICATION_CLOCK_TICK`]), and by nothing of this host's speed — so what a step comes to
/// rest in is the same on an idle host and on a loaded one. What the run may take of THIS host
/// is bounded beside it ([`APPLICATION_HOST_CEILING`]), and no request waits for either: the
/// call that STARTS a run waits only [`APPLICATION_FIRST_WAIT`] for it and the run goes on
/// without it ([`PendingApplicationRun`]).
pub const APPLICATION_RUN_CEILING: Duration = Duration::from_secs(60);

/// How far ONE reading of an application run's clock moves it.
///
/// The span above is spent by what the RUN does, and a traced event is one of those: the engine
/// reads the run's clock once per event the application delivers and once per sample the host's
/// watch takes over it. The tick is sized so that the whole step budget
/// ([`MAX_APPLICATION_STEPS`]) spends well under that span — which is what leaves an
/// application that computes without end to be answered by the step budget, a count of its OWN
/// work, and leaves the span itself for the waits an application asks for.
pub const APPLICATION_CLOCK_TICK: Duration = Duration::from_nanos(100);

/// How much of THIS HOST's own time ONE run of an application may take, past which the engine
/// stops it whatever its own clock reads.
///
/// CHOSEN: the span above is the RUN's own time, so it bounds no CPU of this host — an
/// application spending traced events, or sitting in one long native call that delivers none,
/// would hold a thread of this process for as long as the host took over it. This is the bound
/// that stops it: the engine reads it from this host's own clock at every traced event and from
/// the host's own watch over the run ([`python::Clock::within`]), so a run of caller-supplied
/// code is bounded on this host however its own time is read.
///
/// It is generous beside the spans the engine's phases take in the run's own time
/// ([`::service_com_amazonaws_lambda_awsgirapiservice::state::execution::INIT_PHASE_LIMIT`]
/// standing the execution environment up, then [`APPLICATION_RUN_CEILING`] of application): the
/// interpreter traces every event it executes, so the aggregate a submitted job computes over
/// an input of the size a client really submits takes this host several times as long as the
/// run's own span, and an application that FINISHED must be reported on what it came to rather
/// than on how fast this host read it.
pub const APPLICATION_HOST_CEILING: Duration =
    ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::INIT_PHASE_LIMIT
        .saturating_add(APPLICATION_RUN_CEILING)
        .saturating_mul(HOST_CEILING_OVER_THE_ENGINE_SPAN);

/// How many times the engine's own spans the host ceiling allows for
/// ([`APPLICATION_HOST_CEILING`]).
const HOST_CEILING_OVER_THE_ENGINE_SPAN: u32 = 3;

/// How long after the host time one run may take this emulator still waits for its thread: the
/// engine's stop is latched by the host and raised at the next traced event the interpreter
/// delivers, so a thread it has stopped answers a moment after the ceiling rather than at it.
pub const APPLICATION_ABANDON_GRACE: Duration = Duration::from_secs(10);

/// How long the call that STARTS a run waits for it before leaving it to run on.
///
/// CHOSEN: short, because the run is caller-supplied code and no client's call may be held for
/// it. A run that answers inside this slice decides its step on the call that started it —
/// which is every application small enough to finish at once — and one that does not stays
/// in flight, with the step still `RUNNING`, until a later call collects its answer
/// ([`PendingApplicationRun`]).
pub const APPLICATION_FIRST_WAIT: Duration = Duration::from_secs(2);

/// How many applications this process runs AT ONCE.
///
/// CHOSEN: a run is an interpreter, a thread and the memory the two hold, so the number of
/// them this process carries is bounded rather than following from how many steps a caller
/// submits. The permit is held by the run's own thread and given back when that thread ends.
/// A step that found none waits for one rather than being reported as a step nothing ran
/// ([`StartAttempt::NoInterpreter`]), so this bound decides how long a busy process takes to
/// answer every owed run and never what any one step ends in.
pub const MAX_CONCURRENT_APPLICATION_RUNS: usize = 4;

/// How long a run waits for one of [`MAX_CONCURRENT_APPLICATION_RUNS`] to come free before
/// nothing is run for it. CHOSEN: a bounded wait, so the operation that performs the run
/// answers either way; a step whose run found no interpreter stays owed one and the next
/// call performs it.
pub const APPLICATION_PERMIT_WAIT: Duration = Duration::from_secs(2);

/// How many owed applications ONE call performs, leaving the rest to the next call. CHOSEN:
/// the ceiling on what one REQUEST costs, beside the ceiling on what one run costs.
pub const MAX_APPLICATION_RUNS_PER_CALL: usize = 2;

/// How many runs that ANSWERED NOTHING one step's application is given before the step is
/// ended anyway ([`StepRecord::abandoned_runs`]).
///
/// A run this process gave up waiting for, or whose thread could not be started, is no answer
/// about the submitted work, so the step stays owed its application and a later call runs it.
/// CHOSEN as the bound on that retrying: past this many such runs nothing has answered for the
/// step at all, so leaving it `RUNNING` would hold its cluster's concurrency slot for ever and
/// re-run the application on every later call. The step then ends the way every step this
/// emulator ran no application for ends ([`not_executed`]) — never a status nothing produced.
pub const MAX_ABANDONED_APPLICATION_RUNS: u32 = 3;

/// How long ONE call spends STARTING owed applications before it leaves the rest to the next
/// call.
///
/// CHOSEN: the ceiling on what a REQUEST costs, beside the ceiling on what one run costs. A
/// further application is started only while a whole first wait still fits inside this span,
/// so the longest a reporting operation can be held is this span rather than the product of
/// the per-run ceiling and [`MAX_APPLICATION_RUNS_PER_CALL`].
pub const APPLICATION_CALL_CEILING: Duration = Duration::from_secs(8);

/// The stack an application's thread is given: well above the interpreter's clamped
/// recursion limit, so deep recursion ends the run with the interpreter's own
/// `RecursionError` rather than touching the thread's guard page.
const APPLICATION_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How many traced events one application may execute. CHOSEN: large enough for the
/// transformations a submitted job computes — the interpreter traces every event, so this is
/// the budget that has to cover an aggregate over an input of the size a client really submits
/// — and small enough that the whole of it spends well under the run's own span
/// ([`APPLICATION_CLOCK_TICK`]), so an application that computes without end is answered by
/// THIS budget, a count of its own work, rather than by a span of time.
const MAX_APPLICATION_STEPS: u64 = 200_000_000;

/// How much memory one application may add to this process. CHOSEN, for the same reason.
const MAX_APPLICATION_MEMORY_BYTES: u64 = 512 * 1024 * 1024;

/// How much one application may write to the `/tmp` of the environment it runs in. CHOSEN,
/// for the same reason: that directory is a disk on a real cluster and is held in this
/// process's memory here, so what one run leaves there is bounded like everything else it
/// holds.
const MAX_APPLICATION_SCRATCH_BYTES: u64 = 512 * 1024 * 1024;

/// How deep one application may recurse. The interpreter's own default is below what the
/// frozen standard library's import chain needs, so this is the limit a run is given.
const MAX_APPLICATION_RECURSION: usize = 2_000;

/// How much of an application's output is kept, and how large a value it may answer with.
///
/// The application's own console streams reach the step's LOG FILES
/// ([`super::step_logs`]) rather than the wire: each is bounded inside the run
/// ([`APPLICATION_STREAM_CHARACTERS`]) and the two travel back in the document the run
/// answers with, which is what the ceiling below admits — the escaping of two bounded
/// streams, with room to spare. The log budget bounds what the interpreter's own runtime
/// writes beside them.
const MAX_APPLICATION_LOG_BYTES: usize = 64 * 1024;
/// See [`MAX_APPLICATION_LOG_BYTES`].
const MAX_APPLICATION_ANSWER_BYTES: usize = 1024 * 1024;

/// The permits [`MAX_CONCURRENT_APPLICATION_RUNS`] hands out, for the whole process.
///
/// Held as an owned permit MOVED INTO the thread that runs the application, so it is given
/// back when that thread really ends — including a run the caller has already given up
/// waiting for, which would otherwise take this bound past its count while its thread was
/// still going.
static APPLICATION_RUN_PERMITS: ::std::sync::LazyLock<::std::sync::Arc<::tokio::sync::Semaphore>> =
    ::std::sync::LazyLock::new(|| {
        ::std::sync::Arc::new(::tokio::sync::Semaphore::new(
            MAX_CONCURRENT_APPLICATION_RUNS,
        ))
    });

/// `FailureDetails$Reason` of a step whose application RAN and failed: the class of the
/// failure, beside the application's own error as the message
/// ([`application_failure_message`]).
pub const APPLICATION_FAILED_REASON: &str =
    "The application the step submitted exited with a non-zero status.";

/// `FailureDetails$Message` of such a step: the application's own error, as the engine that
/// ran it reported the exception — this emulator's words appear nowhere in it.
#[must_use]
pub fn application_failure_message(error_type: &str, error_message: &str) -> String {
    if error_message.is_empty() {
        return error_type.to_string();
    }
    format!("{error_type}: {error_message}")
}

/// `FailureDetails$Message` of a step whose application RAN and exited with a non-zero status
/// without raising: the status it exited with, which is the whole of what it reported.
#[must_use]
pub fn application_exit_message(status: i32) -> String {
    format!("The application the step submitted exited with status {status}.")
}

/// `FailureDetails$Reason` of a step whose application ran past a bound of this emulator.
pub const APPLICATION_BOUND_REASON: &str =
    "The application the step submitted ran past a bound of this emulator.";

/// `FailureDetails$Message` of such a step: which bound stopped the run.
#[must_use]
pub fn application_bound_message(bound: &str) -> String {
    format!("The application was stopped because it ran past {bound}.")
}

/// `FailureDetails$Reason` of a step whose application could not be READ at all.
pub const APPLICATION_UNREADABLE_REASON: &str =
    "The application the step submitted could not be read.";

/// `FailureDetails$Message` of such a step: which object could not be read, and what the
/// object store answered about it.
#[must_use]
pub fn application_unreadable_message(path: &str, detail: &str) -> String {
    format!("Unable to download the application {path}: {detail}")
}

/// One application this service owes a run: which step of which cluster submitted it, what it
/// is, and the role its own calls are made as.
#[derive(Debug, Clone)]
pub struct StepApplicationPlan {
    /// The step that submitted it.
    pub step_id: String,
    /// The application the step's command named.
    pub application: SparkApplication,
    /// The step's own runtime role, where it carries one ("the Amazon Resource Name (ARN) of
    /// the runtime role for a step on the cluster", `Step$ExecutionRoleArn`), which is what
    /// the application's calls are then signed as. A step carrying none presents what the
    /// cluster's instances obtain from their instance profile instead
    /// ([`cluster_step_credentials`]).
    pub execution_role_arn: Option<String>,
    /// The version of Spark the cluster's release ships, which is what the application is
    /// told it runs on — `None` for a cluster whose release bundles no Spark.
    pub spark_version: Option<String>,
    /// The Spark properties the CLUSTER's own configuration sets, which are the defaults the
    /// application's configuration starts with ([`spark_default_properties`]).
    pub settings: Vec<(String, String)>,
}

impl ClusterRecord {
    /// Every application this cluster owes a run, in queue order.
    ///
    /// A step owes one once the cluster has STARTED it and nothing has answered for it yet
    /// ([`StepRecord::awaits_application`]); a step that has not started, has already been
    /// answered for, or is stopping owes nothing.
    fn owed_step_applications(&self) -> Vec<StepApplicationPlan> {
        // The engine the cluster runs, as its own release ships it: an application is told
        // the version of Spark it is running on rather than one this emulator spells.
        let spark_version = self
            .release_label
            .as_deref()
            .and_then(|label| release_application_version(label, SPARK_APPLICATION_NAME))
            .map(str::to_string);
        // The Spark properties this cluster's own configuration sets, which every
        // application it runs is submitted with.
        let settings = spark_default_properties(&self.configurations);
        self.steps
            .iter()
            .filter(|step| step.awaits_application() && step.start_date_time.is_some())
            .filter_map(|step| {
                Some(StepApplicationPlan {
                    step_id: step.id.clone(),
                    application: step.application.clone()?,
                    execution_role_arn: step.execution_role_arn.clone(),
                    spark_version: spark_version.clone(),
                    settings: settings.clone(),
                })
            })
            .collect()
    }
}

/// Run every application the cluster `cluster_id` already owes, and record what each
/// answered.
///
/// Called by the operations that report a step or its cluster, BEFORE they take this
/// service's store: the run was owed the instant the cluster started the step, whoever asks
/// about it next, and neither the Amazon S3 read nor the interpreter may happen under this
/// service's lock. The store is taken only to read the owed plans and to write the outcomes.
///
/// Every transition this leads to was owed whether or not anybody called, so a reporting
/// operation may apply it: what it adds is the step's own answer, which is what the step then
/// ends in ([`StepRecord::run_between`]).
pub async fn run_due_step_applications<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    cluster_id: &str,
    now: SystemTime,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let job_flow_role: String = {
        let mut store = state.lock();
        // The queue is run up to this instant first, so a step whose turn has come is
        // STARTED and its application is among the owed ones.
        run_due_steps(engine, &mut store, cluster_id, now);
        match store.clusters.get(cluster_id) {
            Some(cluster) if !cluster.owed_step_applications().is_empty() => {
                cluster.job_flow_role.clone().unwrap_or_default()
            }
            _ => return,
        }
    };
    // ONE runner performs a cluster's owed applications at a time. The plans are read under
    // this store's guard and an application runs with that guard RELEASED — it is
    // caller-supplied code and may not be held under a lock — so two runners that overlapped
    // would both find the same plan owed and run one submitted application TWICE. The claim
    // is what makes it run once, whether the second runner is another client's read or the
    // task driving the cluster's own work ([`super::drive_cluster_work`]); the runner that
    // holds it performs what the cluster owes inside this same window.
    let Some(_claim) = StepApplicationRunnerClaim::take(&state, cluster_id) else {
        return;
    };
    // What the cluster's Amazon EC2 instances obtain from their instance profile, resolved
    // once for this call: the credential an application whose step carries no runtime role of
    // its own presents, which is nothing at all where that profile carries no role those
    // instances may assume.
    let instance_credentials = cluster_step_credentials(ctx, region, &job_flow_role).await;
    // Whether the cluster's Hive metastore is the one this emulator holds, which is what the
    // statements a submitted application's own session resolve their table names in
    // ([`super::spark_session_sql`]). Read once for this call.
    let metastore_is_the_catalog = {
        let store = state.lock();
        match store.clusters.get(cluster_id) {
            Some(cluster) => uses_glue_data_catalog(cluster),
            None => return,
        }
    };
    // The ROLE the cluster's instances run as, resolved once for this call as well: the
    // permissions in force on it are what decide the metastore calls an application's
    // statements make. Nothing resolved is nothing known about those permissions, which
    // withholds no call — and a cluster whose metastore this emulator does not hold resolves
    // nothing in one, so there is no call of it to decide and nothing to resolve.
    let instance_role = match metastore_is_the_catalog {
        true => work_role_arn(ctx, region, &job_flow_role).await,
        false => None,
    };
    // What this CALL may spend starting them: a further application is started only while a
    // whole first wait still fits, so the rest stay owed to the next call.
    let started = ::std::time::Instant::now();
    // The steps this call has already started, collected or given up on, so re-reading the
    // queue below never starts one application twice and never spins on a step nothing ran.
    let mut attempted: HashSet<String> = HashSet::new();
    while attempted.len() < MAX_APPLICATION_RUNS_PER_CALL {
        if started.elapsed().saturating_add(APPLICATION_FIRST_WAIT) > APPLICATION_CALL_CEILING {
            return;
        }
        // The queue is RE-READ every time round rather than listed once: the answer this run
        // gives is what lets its step END, which frees the concurrency slot the step behind
        // it was waiting for — so that step's application is owed inside this same call, and
        // a queue whose steps run one at a time settles in the call that reports it instead
        // of one step per call.
        let next = {
            let mut store = state.lock();
            run_due_steps(engine, &mut store, cluster_id, now);
            store
                .clusters
                .get(cluster_id)
                .map(|cluster| cluster.owed_step_applications())
                .unwrap_or_default()
                .into_iter()
                .find(|plan| !attempted.contains(&plan.step_id))
        };
        let Some(plan) = next else {
            return;
        };
        attempted.insert(plan.step_id.clone());
        let key = step_application_run_key(cluster_id, &plan.step_id);
        // A run this process has already STARTED for the step is not started again: its
        // answer is collected if it has one, and otherwise it is left running and the step
        // stays `RUNNING` — so one application runs once however often its step is read.
        let in_flight = state.lock().take_step_application_run(&key);
        let answered = match in_flight {
            Some(pending) => match collect_application_run(pending, now) {
                CollectedRun::Answered(outcome) => Some(outcome),
                CollectedRun::Waiting(pending) => {
                    state.lock().hold_step_application_run(key, pending);
                    continue;
                }
                CollectedRun::NoAnswer => None,
            },
            None => {
                // A step's own runtime role is what its application's calls are signed as
                // where it carries one; every other step presents what the cluster's
                // instances obtained.
                let credentials = match plan.execution_role_arn.as_deref() {
                    Some(role_arn) => StepCredentials::Role(role_arn.to_string()),
                    None => instance_credentials.clone(),
                };
                // And the role the statements its session submits reach the metastore as,
                // which is the same role: its own runtime role where it carries one. A cluster
                // with no metastore of this emulator's resolves nothing in one, so no role of
                // it is resolved either.
                let metastore = SessionMetastore {
                    is_the_catalog: metastore_is_the_catalog,
                    work_role_arn: match plan.execution_role_arn.as_deref() {
                        Some(role_arn) if metastore_is_the_catalog => {
                            work_role_arn(ctx, region, role_arn).await
                        }
                        Some(_) => None,
                        None => instance_role.clone(),
                    },
                };
                match run_step_application(
                    engine,
                    ctx,
                    region,
                    &plan,
                    &credentials,
                    &metastore,
                    now,
                )
                .await
                {
                    StartAttempt::Started(StartedApplication::Answered(outcome)) => Some(outcome),
                    StartAttempt::Started(StartedApplication::Pending(pending)) => {
                        state.lock().hold_step_application_run(key, pending);
                        continue;
                    }
                    // NOTHING of the application ran: no interpreter of this process was
                    // free for it. That is a fact about this process's load and no answer
                    // about the submitted work, so the step stays owed its run for the next
                    // call to make and is NOT counted as a run that answered nothing.
                    StartAttempt::NoInterpreter => continue,
                    StartAttempt::NoAnswer => None,
                }
            }
        };
        let Some(outcome) = answered else {
            // Nothing answered: the step stays owed its application, so a later call runs it
            // rather than this one reporting an outcome nothing produced — but the ATTEMPT is
            // counted, so a run that never answers cannot leave the step `RUNNING` for good
            // ([`MAX_ABANDONED_APPLICATION_RUNS`]).
            let mut store = state.lock();
            if let Some(step) = store
                .clusters
                .get_mut(cluster_id)
                .and_then(|cluster| cluster.step_mut(&plan.step_id))
            {
                if step.awaits_application() {
                    step.abandoned_runs = step.abandoned_runs.saturating_add(1);
                    if step.abandoned_runs >= MAX_ABANDONED_APPLICATION_RUNS {
                        // That many runs have answered nothing at all, so nothing is going to:
                        // the step ends the way every step this emulator ran no application
                        // for ends, rather than staying `RUNNING` and holding its cluster's
                        // concurrency slot for ever.
                        step.outcome = Some(not_executed());
                    }
                }
            }
            run_due_steps(engine, &mut store, cluster_id, now);
            continue;
        };
        let mut store = state.lock();
        if let Some(step) = store
            .clusters
            .get_mut(cluster_id)
            .and_then(|cluster| cluster.step_mut(&plan.step_id))
        {
            // Only a step that STILL awaits its application takes this answer: in the window
            // the guard was released the caller may have cancelled the step or its cluster,
            // and neither is overwritten by an outcome computed before that happened.
            if step.awaits_application() {
                step.outcome = Some(outcome);
            }
        }
        // The step can now reach the state its application answered, if its span has
        // already passed.
        run_due_steps(engine, &mut store, cluster_id, now);
    }
}

/// The key the run of ONE step's application is held under while it is in flight: the step and
/// the cluster that submitted it, which is what addresses that step.
#[must_use]
pub fn step_application_run_key(cluster_id: &str, step_id: &str) -> String {
    format!("{cluster_id}\u{1f}{step_id}")
}

/// A run this process STARTED and that has not answered yet.
///
/// The run itself is a thread of this process ([`run_application`]); what is held here is
/// where its answer arrives and when this emulator gives up waiting for one. It is held in the
/// SERVICE STATE rather than in a process-wide table, so a run belongs to the store whose step
/// submitted it.
#[derive(Debug)]
pub struct PendingApplicationRun {
    /// Where the run's own answer arrives, once its thread has one.
    answer: ::tokio::sync::oneshot::Receiver<python::Finished>,
    /// When this emulator gives up on it: the host time one run may take plus the grace its
    /// stop needs to land ([`APPLICATION_HOST_CEILING`], [`APPLICATION_ABANDON_GRACE`]). A run
    /// that has not answered by then is one nothing is known about.
    abandon_at: SystemTime,
}

/// What a step's in-flight run has come to.
enum CollectedRun {
    /// The run answered, and this is what its step ends in.
    Answered(StepOutcome),
    /// The run is still going: the step stays `RUNNING`, so no state is reported for work
    /// that has not finished.
    Waiting(PendingApplicationRun),
    /// Nothing is known about what the application came to — this emulator gave up waiting
    /// for it, or its thread ended without reporting. The step stays OWED its application.
    NoAnswer,
}

/// What a step's in-flight run has come to, and the run itself back when it is still going.
fn collect_application_run(mut pending: PendingApplicationRun, now: SystemTime) -> CollectedRun {
    match pending.answer.try_recv() {
        Ok(finished) => CollectedRun::Answered(read_outcome(&finished.completion)),
        Err(::tokio::sync::oneshot::error::TryRecvError::Empty) if now < pending.abandon_at => {
            CollectedRun::Waiting(pending)
        }
        Err(_) => CollectedRun::NoAnswer,
    }
}

impl ComAmazonawsEmrElasticmapreduceState {
    /// Take the in-flight run held under `key`, if this store holds one.
    ///
    /// Taken rather than borrowed, because what the caller does with it — collect its answer
    /// or hold it again ([`Self::hold_step_application_run`]) — happens with no guard on this
    /// store.
    fn take_step_application_run(&mut self, key: &str) -> Option<PendingApplicationRun> {
        self.step_application_runs.remove(key)
    }

    /// Hold `pending` under `key` until a later call collects its answer.
    fn hold_step_application_run(&mut self, key: String, pending: PendingApplicationRun) {
        self.step_application_runs.insert(key, pending);
    }
}

/// The claim ONE runner of a cluster's owed applications holds while it performs them.
///
/// Taken for the whole of [`run_due_step_applications`] and given back however that function
/// ends — which is what a `Drop` claim is for, since it ends at several places — so a cluster
/// never has two runners starting the same step's application.
struct StepApplicationRunnerClaim {
    state: ::engine::ServiceState<ComAmazonawsEmrElasticmapreduceState>,
    cluster_id: String,
}

impl StepApplicationRunnerClaim {
    /// The claim on `cluster_id`, or `None` when another runner already holds it.
    fn take(
        state: &::engine::ServiceState<ComAmazonawsEmrElasticmapreduceState>,
        cluster_id: &str,
    ) -> Option<Self> {
        if !state
            .lock()
            .step_application_runners
            .insert(cluster_id.to_string())
        {
            return None;
        }
        Some(Self {
            state: ::engine::ServiceState::clone(state),
            cluster_id: cluster_id.to_string(),
        })
    }
}

impl Drop for StepApplicationRunnerClaim {
    fn drop(&mut self) {
        self.state
            .lock()
            .step_application_runners
            .remove(&self.cluster_id);
    }
}

/// What starting one application came to: an answer already, or a run still going.
enum StartedApplication {
    /// The run answered inside the slice the call that started it waited, or there was
    /// nothing to run and the step fails for that.
    Answered(StepOutcome),
    /// The run is still going, and its answer is collected by a later call.
    Pending(PendingApplicationRun),
}

/// What ASKING this process to start one application came to.
enum StartAttempt {
    /// It was started, and this is where it got to.
    Started(StartedApplication),
    /// No interpreter of this process was free inside [`APPLICATION_PERMIT_WAIT`], so nothing
    /// of the application ran at all. That is a fact about this process's load rather than an
    /// attempt at the submitted work, so the step stays OWED its run and nothing is counted
    /// against it.
    NoInterpreter,
    /// The run was started and answered nothing: its thread could not be spawned, or it ended
    /// without reporting. Nothing is known about what the application came to.
    NoAnswer,
}

/// What the SESSION of a submitted application resolves its statements against: whether the
/// cluster's Hive metastore is the AWS Glue Data Catalog this emulator holds
/// ([`uses_glue_data_catalog`]), and the role its statements reach that metastore as.
///
/// Both are the CLUSTER's, so both are resolved once for a call that runs what the cluster owes
/// rather than per application ([`run_due_step_applications`]).
#[derive(Debug, Clone)]
struct SessionMetastore {
    /// Whether the catalog this emulator holds is that metastore.
    is_the_catalog: bool,
    /// The role the statements reach it as, and `None` where nothing is known about one —
    /// which withholds no call.
    work_role_arn: Option<String>,
}

/// Start ONE owed application and answer what its step ends in, the run itself when it is
/// still going, or why nothing of it ran.
async fn run_step_application<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    plan: &StepApplicationPlan,
    credentials: &StepCredentials,
    metastore: &SessionMetastore,
    now: SystemTime,
) -> StartAttempt {
    let source = match read_application(ctx, region, &plan.application).await {
        Ok(source) => source,
        // The application's own file could not be read, which is a failure of the step
        // itself: there is nothing to run, and the step says exactly that.
        Err(detail) => {
            return StartAttempt::Started(StartedApplication::Answered(failed(
                APPLICATION_UNREADABLE_REASON,
                application_unreadable_message(&plan.application.path, &detail),
            )))
        }
    };
    let permit = match ::tokio::time::timeout(
        APPLICATION_PERMIT_WAIT,
        ::std::sync::Arc::clone(&APPLICATION_RUN_PERMITS).acquire_owned(),
    )
    .await
    {
        Ok(Ok(permit)) => permit,
        _ => return StartAttempt::NoInterpreter,
    };
    // The calls the application itself makes present what the step runs with, over the same
    // in-process callback, and the application never sees the credential. An environment that
    // holds NO credential is given no seam at all: a call the application makes is then made
    // by nothing, rather than signed with a credential the cluster's instances do not hold.
    let access = match credentials {
        StepCredentials::Role(role_arn) => {
            function_sdk_access(engine, ctx, region, role_arn, &plan.step_id)
        }
        StepCredentials::Absent => None,
    };
    // WHERE the statements the application's own session submits are planned and run: the
    // cluster's Hive metastore and the data its tables point at, read as the role the step runs
    // as — the same engine a step's SQL client runs a statement on. It travels with the seam
    // above because it is the same thing: what this run reaches of the rest of this emulator.
    // A cluster whose metastore this emulator does not hold reads no relation's data and makes
    // no call of that metastore, so neither the program that reads it nor the decision over it
    // is built for one.
    let access = access.map(|access| {
        access.running_statements_on(Some(::std::sync::Arc::new(spark_session_sql(
            engine,
            metastore.is_the_catalog,
            metastore
                .is_the_catalog
                .then(|| step_object_cli(engine, ctx, region, credentials, &plan.step_id))
                .flatten(),
            metastore
                .work_role_arn
                .as_deref()
                .and_then(|role_arn| step_catalog_access(ctx, region, role_arn, &plan.step_id)),
        ))))
    });
    match run_application(plan, &source, access, permit, now).await {
        Some(started) => StartAttempt::Started(started),
        None => StartAttempt::NoAnswer,
    }
}

/// The application's own source, read from the object its path names.
///
/// The read is RANGED, so at most [`MAX_APPLICATION_BYTES`] plus the one byte that shows the
/// object is past the bound is ever buffered, and a file that is not text is one no
/// interpreter can compile.
async fn read_application(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    application: &SparkApplication,
) -> Result<String, String> {
    let read = ::service_common::s3_client::s3_client(ctx.callback(), region)
        .get_object()
        .bucket(&application.script.bucket)
        .key(&application.script.key)
        .range(format!("bytes=0-{MAX_APPLICATION_BYTES}"))
        .send()
        .await
        .map_err(|error| {
            format!(
                "the object store answered {:?}: {}",
                error.code(),
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    let body = read
        .body
        .collect()
        .await
        .map_err(|error| format!("the application did not arrive in full: {error}"))?
        .into_bytes();
    if body.len() > MAX_APPLICATION_BYTES {
        return Err(format!(
            "the application is past the {MAX_APPLICATION_BYTES} bytes one may hold"
        ));
    }
    String::from_utf8(body.to_vec()).map_err(|_| "the application is not text".to_string())
}

/// What one run of an application may spend.
#[must_use]
pub fn application_budget() -> python::Budget {
    python::Budget {
        wall_clock: APPLICATION_RUN_CEILING,
        steps: MAX_APPLICATION_STEPS,
        memory_bytes: MAX_APPLICATION_MEMORY_BYTES,
        ephemeral_bytes: MAX_APPLICATION_SCRATCH_BYTES,
        log_bytes: MAX_APPLICATION_LOG_BYTES,
        response_bytes: MAX_APPLICATION_ANSWER_BYTES,
        recursion_limit: MAX_APPLICATION_RECURSION,
    }
}

/// Run `source` as the step's application, on a thread of its own, and answer what the step
/// ends in, the run that is still going, or `None` when the run ANSWERED NOTHING at all.
///
/// The interpreter is synchronous and CPU-bound, so it runs on a thread with a stack of its
/// own rather than on the async runtime. The CALLER's wait for it is [`APPLICATION_FIRST_WAIT`]
/// and no longer: a run that has not answered by then goes on without it, and its step stays
/// `RUNNING` until a later call collects the answer. `permit` is this run's share of
/// [`MAX_CONCURRENT_APPLICATION_RUNS`] and is MOVED into that thread, so it comes back when
/// the thread ends — including a thread nobody is waiting for any more.
///
/// A run whose thread could not be started at all, and one whose thread ended without
/// reporting, is `None`: nothing is known about what the application came to, so the step
/// stays OWED its application and a later call runs it — exactly as a command nothing
/// answered for does ([`super::command_step_run`]). Reporting it as a step whose application
/// was not started would settle the step on this process's own timing rather than on anything
/// the submitted work answered. The run's OWN bounds are unaffected: an application that
/// spends its budget is reported as stopped by the interpreter, which IS an answer.
async fn run_application(
    plan: &StepApplicationPlan,
    source: &str,
    access: Option<AwsAccess>,
    permit: ::tokio::sync::OwnedSemaphorePermit,
    now: SystemTime,
) -> Option<StartedApplication> {
    let budget = application_budget();
    let event = ::serde_json::json!({
        "path": plan.application.path,
        "source": source,
        "arguments": plan.application.arguments,
        "streamCharacters": APPLICATION_STREAM_CHARACTERS,
    })
    .to_string();
    let step_id = plan.step_id.clone();
    let plan_spark_version = plan.spark_version.clone();
    let plan_settings = plan.settings.clone();
    let (sender, mut receiver) = ::tokio::sync::oneshot::channel();
    let spawned = ::std::thread::Builder::new()
        .name("emr-spark-application".to_string())
        .stack_size(APPLICATION_STACK_BYTES)
        .spawn(move || {
            // Held for as long as the application runs, whoever is still waiting for it.
            let _permit = permit;
            let modules = spark_runtime_modules(plan_spark_version.as_deref(), &plan_settings);
            let handler = application_entry_handler();
            let finished = python::run_against(
                &python::Run {
                    modules: &modules,
                    handler: &handler,
                    // A Spark application reads its parameters off its command line, so the
                    // run is given no environment of its own.
                    environment: &BTreeMap::new(),
                    // The instruction set the environment the application runs in has, which
                    // the run reports as its own machine. A step names none, so it is the one
                    // the engine's own runtime reports when none is named.
                    architecture:
                        ::service_com_amazonaws_lambda_awsgirapiservice::state::DEFAULT_ARCHITECTURE,
                    event: Some(&event),
                    request_id: &step_id,
                    invoked_function_arn: "",
                    client_context: None,
                    tenant_id: None,
                    // Nor an Amazon Cognito identity: a Spark application is this service's
                    // own run, made by no identity a pool vended credentials to.
                    cognito_identity: None,
                    budget,
                    aws: access.as_ref(),
                    // A Spark step is no Lambda durable execution's run, so it takes no step
                    // of one and checkpoints nothing onto one.
                    durable: None,
                    // And it mounts no Amazon EFS file system: the files the application
                    // reaches are the cluster's own, which this service holds.
                    file_systems: &[],
                },
                // The clock the run's own span is read against: one the RUN drives, a tick per
                // traced event it delivers and the waits it asks for, so the state the STEP
                // comes to rest in is what the application came to rather than what this
                // host's load made of it — stating what the run may take of THIS host besides,
                // which is the bound the engine stops it on whatever its own clock reads. The
                // waits it asks for really PASS ([`python::Clock::waiting`]): a step is
                // observed by a client while it is in flight, so an application that waits
                // keeps its step `RUNNING` for that span and the effects it makes after
                // waiting are made after it.
                &python::Clock::driven(APPLICATION_CLOCK_TICK)
                    .waiting()
                    .within(APPLICATION_HOST_CEILING),
            );
            let _ = sender.send(finished);
        });
    if spawned.is_err() {
        return None;
    }
    match ::tokio::time::timeout(APPLICATION_FIRST_WAIT, &mut receiver).await {
        Ok(Ok(finished)) => Some(StartedApplication::Answered(read_outcome(
            &finished.completion,
        ))),
        // The thread ended without reporting: nothing is known about what the application
        // came to, so the step stays owed it rather than being settled on an outcome nothing
        // produced.
        Ok(Err(_)) => None,
        // Still running, which is what every application larger than a moment's work is: the
        // run goes on and a later call collects its answer. The instant this emulator gives
        // up on it is reached by CHECKED addition, so an instant no clock can hold is one it
        // gives up on at once rather than one it faults on.
        Err(_) => Some(StartedApplication::Pending(PendingApplicationRun {
            answer: receiver,
            abandon_at: now
                .checked_add(APPLICATION_HOST_CEILING.saturating_add(APPLICATION_ABANDON_GRACE))
                .unwrap_or(now),
        })),
    }
}

/// What the step ends in, given what the interpreter answered.
fn read_outcome(completion: &python::Completion) -> StepOutcome {
    match completion {
        // The entry module REPORTS what the run came to ([`super::spark_runtime`]): the
        // status the application exited with, the two console streams it wrote, and the
        // exception it ended on when it ended on one.
        python::Completion::Returned(answered) => reported_run(answered),
        // A streaming answer is one nothing here asks for, so a run that produced one is a
        // run this service can say nothing about.
        python::Completion::Streamed { .. } => not_executed(),
        // The entry module itself raised, which is not the application's own failure: the
        // engine reports the exception, and what it ran is still what failed.
        python::Completion::Raised {
            error_type,
            error_message,
            ..
        } => failed(
            APPLICATION_FAILED_REASON,
            application_failure_message(error_type, error_message),
        ),
        python::Completion::Stopped(stop) => failed(
            APPLICATION_BOUND_REASON,
            application_bound_message(match stop {
                python::Stop::Deadline => "the wall clock this emulator gives an application",
                python::Stop::Steps => "the step budget this emulator gives an application",
                python::Stop::Memory => "the memory ceiling this emulator gives an application",
                python::Stop::Response => {
                    "the size of the value an application may answer with"
                }
                python::Stop::HostCeiling => {
                    "the host time this emulator gives one run of an application"
                }
            }),
        ),
        // The interpreter itself could not be built or driven: that is not the
        // application's failure, so nothing is claimed about it.
        python::Completion::Internal(_) => not_executed(),
    }
}

/// What the step ends in, given the document the entry module reported the run in.
///
/// "For a step to be considered complete, the main function must exit with a zero exit code"
/// (`AddJobFlowSteps` doc), so the status the application exited with IS the step's end: zero
/// is its completion and anything else the modelled failure state, carrying the application's
/// own exception where it ended on one and the status it exited with where it did not. Either
/// way the run's console streams and that status are kept for the step's log files
/// ([`StepRunLog`]).
///
/// A document this service cannot read is one nothing is known from, so the step ends the way
/// every step this emulator ran no application for ends.
fn reported_run(answered: &str) -> StepOutcome {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(answered) else {
        return not_executed();
    };
    let Some(status) = document
        .get("exitStatus")
        .and_then(::serde_json::Value::as_i64)
        .and_then(|status| i32::try_from(status).ok())
    else {
        return not_executed();
    };
    let text = |member: &str| {
        document
            .get(member)
            .and_then(::serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let log = StepRunLog {
        stdout: text("stdout"),
        stderr: text("stderr"),
        exit_status: Some(status),
    }
    .bounded();
    if status == 0 {
        return StepOutcome {
            state: step_state::COMPLETED.to_string(),
            failure: None,
            log: Some(log),
        };
    }
    let error_type = text("errorType");
    let message = if error_type.is_empty() {
        application_exit_message(status)
    } else {
        application_failure_message(&error_type, &text("errorMessage"))
    };
    StepOutcome {
        log: Some(log),
        ..failed(APPLICATION_FAILED_REASON, message)
    }
}

/// The step's end for an application that FAILED, in the two members the model gives a step
/// failure.
fn failed(reason: &str, message: String) -> StepOutcome {
    StepOutcome {
        state: step_state::FAILED.to_string(),
        failure: Some(StepFailure {
            reason: reason.to_string(),
            message,
        }),
        log: None,
    }
}

/// The step's end when nothing ran its work and nothing can be said about it: the modelled
/// failure state with the reason every step this emulator ran no application for carries
/// ([`STEP_NOT_EXECUTED_REASON`]).
///
/// Shared with the command queue ([`super::command_step_run`]), whose commands end the same
/// way when the session performed nothing of them, so the two have ONE spelling of this end.
pub fn not_executed() -> StepOutcome {
    StepOutcome {
        state: step_state::FAILED.to_string(),
        failure: Some(StepFailure {
            reason: STEP_NOT_EXECUTED_REASON.to_string(),
            message: STEP_NOT_EXECUTED_MESSAGE.to_string(),
        }),
        // Nothing ran, so nothing wrote a console stream and nothing exited with a status.
        log: None,
    }
}
