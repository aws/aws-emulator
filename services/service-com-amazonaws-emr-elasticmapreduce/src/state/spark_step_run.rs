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
//! ([`ClusterRecord::advance_to`]) — and it is performed by the next operation that reports
//! that step or its cluster, with no store guard held, exactly as a Glue job run's script is
//! performed by the next call that reads the run. A step whose application has not run yet
//! does not END ([`StepRecord::awaits_application`]): the state it ends in is the one its
//! application answered, so the step stays `RUNNING` until there is an answer, and no step
//! is ever reported COMPLETED for work nothing performed.
//!
//! # Bounds
//!
//! An application is caller-supplied code, so every bound is this emulator's and none of them
//! is reachable from the application: how many applications this process runs at once
//! ([`MAX_CONCURRENT_APPLICATION_RUNS`], with a bounded wait for a permit), how many ONE call
//! performs ([`MAX_APPLICATION_RUNS_PER_CALL`]), how large the application may be
//! ([`MAX_APPLICATION_BYTES`], enforced by the ranged read), and the wall clock, step,
//! memory, recursion and output ceilings the interpreter is given
//! ([`application_budget`]). The containment — the native modules that are never registered,
//! the host surface that is compiled out and the stop no guest call can clear — is that
//! engine's, which is why nothing of it is stood up again here.

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

/// The longest ONE run of a submitted application may take.
///
/// CHOSEN, not an AWS duration: a real Spark application runs for as long as its cluster
/// gives it, and here the operation that reports the step waits for the run — so this is the
/// wall clock an application gets, and an application that runs past it fails the step on
/// that bound rather than being left running.
pub const APPLICATION_RUN_CEILING: Duration = Duration::from_secs(5);

/// How long after its budget is spent an application's thread is still waited for: the
/// interpreter's stop lands at the next traced event, so the answer of a run that was stopped
/// arrives a little after its wall clock rather than at it.
pub const APPLICATION_ABANDON_GRACE: Duration = Duration::from_secs(2);

/// How many applications this process runs AT ONCE.
///
/// CHOSEN: a run is an interpreter, a thread and the memory the two hold, so the number of
/// them this process carries is bounded rather than following from how many steps a caller
/// submits. The permit is held by the run's own thread and given back when that thread ends.
pub const MAX_CONCURRENT_APPLICATION_RUNS: usize = 2;

/// How long a run waits for one of [`MAX_CONCURRENT_APPLICATION_RUNS`] to come free before
/// nothing is run for it. CHOSEN: a bounded wait, so the operation that performs the run
/// answers either way; a step whose run found no interpreter stays owed one and the next
/// call performs it.
pub const APPLICATION_PERMIT_WAIT: Duration = Duration::from_secs(2);

/// How many owed applications ONE call performs, leaving the rest to the next call. CHOSEN:
/// the ceiling on what one REQUEST costs, beside the ceiling on what one run costs.
pub const MAX_APPLICATION_RUNS_PER_CALL: usize = 2;

/// How long ONE call spends performing owed applications before it leaves the rest to the
/// next call.
///
/// CHOSEN: the ceiling on what a REQUEST costs, beside the ceiling on what one run costs. A
/// further application is started only while a whole run still fits inside this span, so the
/// longest a reporting operation can be held is this span plus the grace one run is given to
/// stop ([`APPLICATION_ABANDON_GRACE`]) rather than the product of the per-run ceiling and
/// [`MAX_APPLICATION_RUNS_PER_CALL`].
pub const APPLICATION_CALL_CEILING: Duration = Duration::from_secs(8);

/// The stack an application's thread is given: well above the interpreter's clamped
/// recursion limit, so deep recursion ends the run with the interpreter's own
/// `RecursionError` rather than touching the thread's guard page.
const APPLICATION_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How many traced events one application may execute. CHOSEN: large enough for the
/// transformations a submitted job computes, small enough that a loop with no exit ends
/// inside the wall clock above.
const MAX_APPLICATION_STEPS: u64 = 20_000_000;

/// How much memory one application may add to this process. CHOSEN, for the same reason.
const MAX_APPLICATION_MEMORY_BYTES: u64 = 512 * 1024 * 1024;

/// How deep one application may recurse. The interpreter's own default is below what the
/// frozen standard library's import chain needs, so this is the limit a run is given.
const MAX_APPLICATION_RECURSION: usize = 2_000;

/// How much of an application's output is kept, and how large a value it may answer with.
/// Neither reaches the wire: a step reports no application output.
const MAX_APPLICATION_LOG_BYTES: usize = 64 * 1024;
/// See [`MAX_APPLICATION_LOG_BYTES`].
const MAX_APPLICATION_ANSWER_BYTES: usize = 64 * 1024;

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
    /// The role the application's own AWS calls are signed as: the step's runtime role where
    /// it has one ("the Amazon Resource Name (ARN) of the runtime role for a step on the
    /// cluster", `Step$ExecutionRoleArn`), and otherwise the role the cluster's instances
    /// assume, which is what runs a step with no runtime role of its own.
    pub role_arn: String,
    /// The version of Spark the cluster's release ships, which is what the application is
    /// told it runs on — `None` for a cluster whose release bundles no Spark.
    pub spark_version: Option<String>,
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
        self.steps
            .iter()
            .filter(|step| step.awaits_application() && step.start_date_time.is_some())
            .filter_map(|step| {
                Some(StepApplicationPlan {
                    step_id: step.id.clone(),
                    application: step.application.clone()?,
                    role_arn: step
                        .execution_role_arn
                        .clone()
                        .or_else(|| self.job_flow_role.clone())
                        .unwrap_or_default(),
                    spark_version: spark_version.clone(),
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
    let owed: Vec<StepApplicationPlan> = {
        let mut store = state.lock();
        // The queue is run up to this instant first, so a step whose turn has come is
        // STARTED and its application is among the owed ones.
        run_due_steps(engine, &mut store, cluster_id, now);
        store
            .clusters
            .get(cluster_id)
            .map(ClusterRecord::owed_step_applications)
            .unwrap_or_default()
    };
    if owed.is_empty() {
        return;
    }
    // What this CALL may spend performing them: a further application is started only while
    // a whole run still fits, so the rest stay owed to the next call.
    let started = ::std::time::Instant::now();
    for plan in owed.into_iter().take(MAX_APPLICATION_RUNS_PER_CALL) {
        if started.elapsed().saturating_add(APPLICATION_RUN_CEILING) > APPLICATION_CALL_CEILING {
            return;
        }
        let Some(outcome) = run_step_application(engine, ctx, region, &plan).await else {
            // Nothing ran: the step stays owed its application, so a later call performs it
            // rather than this one reporting an outcome nothing produced.
            continue;
        };
        let mut store = state.lock();
        if let Some(step) = store
            .clusters
            .get_mut(cluster_id)
            .and_then(|cluster| cluster.step_mut(&plan.step_id))
        {
            step.outcome = Some(outcome);
        }
        // The step can now reach the state its application answered, if its span has
        // already passed.
        run_due_steps(engine, &mut store, cluster_id, now);
    }
}

/// Run ONE owed application and answer what the step ends in, or `None` when nothing ran at
/// all.
async fn run_step_application<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    plan: &StepApplicationPlan,
) -> Option<StepOutcome> {
    let source = match read_application(ctx, region, &plan.application).await {
        Ok(source) => source,
        // The application's own file could not be read, which is a failure of the step
        // itself: there is nothing to run, and the step says exactly that.
        Err(detail) => {
            return Some(failed(
                APPLICATION_UNREADABLE_REASON,
                application_unreadable_message(&plan.application.path, &detail),
            ))
        }
    };
    let permit = ::tokio::time::timeout(
        APPLICATION_PERMIT_WAIT,
        ::std::sync::Arc::clone(&APPLICATION_RUN_PERMITS).acquire_owned(),
    )
    .await
    .ok()?
    .ok()?;
    // The calls the application itself makes are made as the role the step runs as, over the
    // same in-process callback, and the application never sees the credential.
    let access = function_sdk_access(engine, ctx, region, &plan.role_arn, &plan.step_id);
    Some(run_application(plan, &source, access, permit).await)
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
        log_bytes: MAX_APPLICATION_LOG_BYTES,
        response_bytes: MAX_APPLICATION_ANSWER_BYTES,
        recursion_limit: MAX_APPLICATION_RECURSION,
    }
}

/// Run `source` as the step's application, on a thread of its own, and answer what the step
/// ends in.
///
/// The interpreter is synchronous and CPU-bound, so it runs on a thread with a stack of its
/// own rather than on the async runtime, and the caller's wait for it is bounded by the run's
/// own budget plus [`APPLICATION_ABANDON_GRACE`]. `permit` is this run's share of
/// [`MAX_CONCURRENT_APPLICATION_RUNS`] and is MOVED into that thread, so it comes back when
/// the thread ends.
async fn run_application(
    plan: &StepApplicationPlan,
    source: &str,
    access: Option<AwsAccess>,
    permit: ::tokio::sync::OwnedSemaphorePermit,
) -> StepOutcome {
    let budget = application_budget();
    let event = ::serde_json::json!({
        "path": plan.application.path,
        "source": source,
        "arguments": plan.application.arguments,
    })
    .to_string();
    let step_id = plan.step_id.clone();
    let plan_spark_version = plan.spark_version.clone();
    let (sender, receiver) = ::tokio::sync::oneshot::channel();
    let spawned = ::std::thread::Builder::new()
        .name("emr-spark-application".to_string())
        .stack_size(APPLICATION_STACK_BYTES)
        .spawn(move || {
            // Held for as long as the application runs, whoever is still waiting for it.
            let _permit = permit;
            let modules = spark_runtime_modules(plan_spark_version.as_deref());
            let handler = application_entry_handler();
            let finished = python::run(&python::Run {
                modules: &modules,
                handler: &handler,
                // A Spark application reads its parameters off its command line, so the run
                // is given no environment of its own.
                environment: &BTreeMap::new(),
                event: Some(&event),
                request_id: &step_id,
                invoked_function_arn: "",
                client_context: None,
                tenant_id: None,
                budget,
                aws: access.as_ref(),
            });
            let _ = sender.send(finished);
        });
    if let Err(error) = spawned {
        return failed(
            APPLICATION_UNREADABLE_REASON,
            format!("the thread the application runs on could not be started: {error}"),
        );
    }
    let waited = ::tokio::time::timeout(
        budget.wall_clock.saturating_add(APPLICATION_ABANDON_GRACE),
        receiver,
    )
    .await;
    match waited {
        Ok(Ok(finished)) => read_outcome(&finished.completion),
        // The run was given up on, or its thread ended without reporting: nothing is known
        // about what the application came to, so the step ends the way every step this
        // emulator ran no application for ends.
        Ok(Err(_)) | Err(_) => not_executed(),
    }
}

/// What the step ends in, given what the interpreter answered.
fn read_outcome(completion: &python::Completion) -> StepOutcome {
    match completion {
        // An application is a program rather than a function, so what it ANSWERED is not
        // part of the outcome: an application that ran to its end exited cleanly, which is
        // the step's completion ("for a step to be considered complete, the main function
        // must exit with a zero exit code", `AddJobFlowSteps` doc).
        python::Completion::Returned(_) | python::Completion::Streamed { .. } => StepOutcome {
            state: step_state::COMPLETED.to_string(),
            failure: None,
        },
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

/// The step's end for an application that FAILED, in the two members the model gives a step
/// failure.
fn failed(reason: &str, message: String) -> StepOutcome {
    StepOutcome {
        state: step_state::FAILED.to_string(),
        failure: Some(StepFailure {
            reason: reason.to_string(),
            message,
        }),
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
    }
}
