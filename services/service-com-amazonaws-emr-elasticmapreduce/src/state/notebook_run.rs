// SPDX-License-Identifier: CC0-1.0

//! RUNNING a notebook execution's notebook: what the execution's own work answered.
//!
//! One concern of this service's state, split out of [`super::notebook_executions`] the way
//! the cluster's step runner ([`super::step_runner`]) is split out of the step lifecycle:
//! the lifecycle owns the statuses and their spans, and this owns the one thing that needs
//! more than this service's state — whether the notebook the execution names RAN, and what
//! it came to.
//!
//! The notebook is a real document in a real bucket: "the Amazon S3 location that the
//! notebook is stored in" (`StartNotebookExecutionInput$NotebookS3Location` doc, Smithy
//! model), so it is FETCHED from Amazon S3 over the emulator's own in-process callback
//! ([`::service_common::s3_client`]) exactly as any emulated service follows a reference
//! into another, and read as the Jupyter notebook format: its code cells, in order, are the
//! program the execution submits ([`notebook_code_cells`]).
//!
//! Those cells are then EXECUTED, by the contained Python interpreter this emulator already
//! holds for running caller-supplied code
//! ([`::service_com_amazonaws_lambda_awsgirapiservice::state::execution::python`]): one
//! interpreter per run, built with its host-reaching surface compiled OUT, its native
//! modules narrowed to an allow-list and every bound inside the run — deadline, traced
//! steps, memory, recursion depth, output size — enforced in host code the cells cannot
//! reach. The cells reach no other service of this emulator (the run is given no seam to
//! dispatch an AWS call on) and no host path, host environment variable or host credential:
//! the only environment they see is the execution's own `EnvironmentVariables`.
//!
//! Around the run, this module holds what one EXECUTION may cost: the notebook object is
//! bounded before and after it is read ([`MAX_NOTEBOOK_BYTES`]), so is the number of cells
//! ([`MAX_NOTEBOOK_CELLS`]), the runs this process performs at once are a bounded pool
//! ([`MAX_CONCURRENT_NOTEBOOK_RUNS`]) each holding its place until its thread ends, and a run
//! is waited for no longer than its own wall clock and the grace on top of it. Nothing of a
//! run happens on the async executor, nothing of it happens under this service's store lock,
//! and nothing of it happens on the `StartNotebookExecution` request path
//! ([`start_notebook_run`]): that call answers once the cluster has received the request, and
//! the run's answer reaches the execution's record when the run is done.
//!
//! The cells share one set of globals and run in order, which is what a notebook kernel does
//! ([`NOTEBOOK_KERNEL_SOURCE`]), and the execution's end is what that run answered:
//!
//! * every code cell ran without raising — the execution FINISHES;
//! * a cell raised — the execution FAILS, and `LastStateChangeReason` is the exception the
//!   cells produced, which is the engine's own reason for the failure;
//! * the run outlasted one of its bounds — the execution FAILS for that bound;
//! * nothing could be run at all (the execution names no Amazon S3 notebook, Amazon S3
//!   holds no such object, the object is no readable notebook, or its kernel is not a
//!   Python one this emulator can run) — the execution keeps the lifecycle's own
//!   not-executed failure ([`super::NOTEBOOK_NOT_EXECUTED_REASON`]), which says no notebook
//!   was run rather than reporting an outcome for work nothing performed.

use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::python::{
    self, Budget, Completion, Finished, Stop,
};
use smithy_server_runtime::EmbeddedEmulator;

use super::*;
use crate::ops::types::NotebookS3LocationFromInput;

/// The KERNEL a notebook execution's cells run in: the program that compiles and executes
/// each cell, in order, against one set of globals.
///
/// That is what a notebook kernel does — a notebook is a sequence of cells sharing one
/// namespace, so a name a cell binds is a name the next cell sees — and it is all this
/// program does: the cells are the caller's own code, compiled and executed by the
/// interpreter, and the answer is how many of them ran. A cell that raises propagates out
/// of here, so the exception the run reports is the notebook's own.
const NOTEBOOK_KERNEL_SOURCE: &str = r#"
def execute(event, context):
    cells = event.get("cells") or []
    namespace = {"__name__": "__main__"}
    ran = 0
    for index, source in enumerate(cells):
        exec(compile(source, "In[{}]".format(index + 1), "exec"), namespace)
        ran += 1
    return {"cells": ran}
"#;

/// The module the kernel is imported from, and the handler the runtime resolves in it.
const NOTEBOOK_KERNEL_MODULE: &str = "emr_notebook_kernel.py";
const NOTEBOOK_KERNEL_HANDLER: &str = "emr_notebook_kernel.execute";

/// The stack one notebook run is given, on the thread of its own it runs on.
///
/// CHOSEN: the interpreter is recursive over the cells' own syntax, so a run needs more
/// stack than a request thread carries; it is the same order as the stack this emulator
/// gives any other run of caller-supplied code.
const RUN_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How long the cells of one notebook may run for.
///
/// CHOSEN, not documented: a real execution runs for as long as its notebook does, on a
/// cluster. Here it runs off the request path, so the bound is what one execution of this
/// emulator may spend on a notebook — and a notebook that outlasts it is reported as having
/// failed on this bound rather than left running.
const NOTEBOOK_WALL_CLOCK: Duration = Duration::from_secs(5);

/// How much longer than its own wall clock a run is waited for before it is given up on.
///
/// CHOSEN: a run stops at the next event its interpreter delivers after the deadline, and
/// the interpreter is then finalized — the guest's own exit callbacks run under the same
/// bounds — so the answer of a run that was stopped arrives a little after its wall clock
/// rather than at it. The grace covers that, so a notebook stopped by its own bounds is
/// reported as stopped rather than as one nothing could be learned about.
const NOTEBOOK_RUN_GRACE: Duration = Duration::from_secs(5);

/// How many notebook runs this process performs AT ONCE.
///
/// CHOSEN: a run is an interpreter, a thread and the memory the two hold, so the number of
/// them this process carries is bounded rather than following from how many executions a
/// caller starts. The permit is held by the run's own thread and released when that thread
/// ends, so a run that was given up on still counts against the pool for as long as it
/// lasts.
const MAX_CONCURRENT_NOTEBOOK_RUNS: usize = 4;

/// How long a run waits for a place in the pool above to come free before nothing is run for
/// it.
///
/// CHOSEN: nothing is waiting on the answer — `StartNotebookExecution` has already answered,
/// and the execution whose run this is sits at `RUNNING`, "being processed by the cluster",
/// while it queues — so the wait covers two full-length runs ahead of it, each spending its
/// own wall clock and the grace on top of it, which is the longest a place is ever held. It
/// stays SHORT enough that a queued execution still settles inside the wait a client polling
/// it allows: past it, the execution is reported as one nothing ran rather than left queued.
const NOTEBOOK_PERMIT_WAIT: Duration = NOTEBOOK_WALL_CLOCK
    .saturating_add(NOTEBOOK_RUN_GRACE)
    .saturating_mul(2);

/// The runs this process may perform at once ([`MAX_CONCURRENT_NOTEBOOK_RUNS`]).
fn notebook_run_permits() -> &'static ::std::sync::Arc<::tokio::sync::Semaphore> {
    static PERMITS: ::std::sync::OnceLock<::std::sync::Arc<::tokio::sync::Semaphore>> =
        ::std::sync::OnceLock::new();
    PERMITS.get_or_init(|| {
        ::std::sync::Arc::new(::tokio::sync::Semaphore::new(MAX_CONCURRENT_NOTEBOOK_RUNS))
    })
}

/// What one notebook run may spend, beside its wall clock.
///
/// CHOSEN, every one of them: a notebook is caller-supplied code, so each bound is what one
/// execution of this emulator may cost — traced events, the memory the run may add to this
/// process, the output kept, the answer the kernel may return and how deep the cells may
/// recurse. They are enforced by the interpreter's own host-side bounds, which the cells
/// cannot reach.
fn notebook_budget() -> Budget {
    Budget {
        wall_clock: NOTEBOOK_WALL_CLOCK,
        steps: 2_000_000,
        memory_bytes: 256 * 1024 * 1024,
        log_bytes: 16 * 1024,
        response_bytes: 64 * 1024,
        recursion_limit: 2_000,
    }
}

/// How many bytes of a notebook object are read at most.
///
/// CHOSEN: a notebook is a document a person edits, so this is well above any real one and
/// bounds what one request makes this process hold. It is taken TWICE: against the length
/// Amazon S3 reports, before any of the object is held, and against the bytes that actually
/// arrived, so the cap holds whatever the answer claimed.
pub const MAX_NOTEBOOK_BYTES: usize = 16 * 1024 * 1024;

/// How many code cells of one notebook are run.
///
/// CHOSEN, for the same reason the notebook's own size is bounded: the cells are the
/// program one request submits.
pub const MAX_NOTEBOOK_CELLS: usize = 1_000;

/// The code cells of the notebook `document`, in the order the notebook holds them, or
/// `None` when `document` is no notebook this emulator can run.
///
/// The Jupyter notebook format holds its cells in a top-level `cells` array, each with a
/// `cell_type` and a `source` that is either one string or the list of its lines; a cell
/// whose type is not `code` carries no program (a Markdown cell is prose), so only the code
/// cells are the notebook's program. A document with no `cells` array is no notebook, and
/// one whose kernel is not a Python kernel is a notebook this emulator cannot run: the
/// interpreter it runs cells in is a Python one, so a Scala or R notebook is reported as not
/// having been run rather than run as something it is not. A notebook that states no kernel
/// at all is taken as the Python kind, which is what an Amazon EMR notebook's default kernel
/// is.
pub fn notebook_code_cells(document: &[u8]) -> Option<Vec<String>> {
    let notebook: ::serde_json::Value = ::serde_json::from_slice(document).ok()?;
    if !notebook_is_python(&notebook) {
        return None;
    }
    let cells = notebook.get("cells")?.as_array()?;
    Some(
        cells
            .iter()
            .filter(|cell| {
                cell.get("cell_type")
                    .and_then(::serde_json::Value::as_str)
                    .is_some_and(|kind| kind == "code")
            })
            .filter_map(cell_source)
            .take(MAX_NOTEBOOK_CELLS)
            .collect(),
    )
}

/// Whether `notebook`'s kernel is a Python one — the kind whose cells this emulator can
/// run.
///
/// The notebook format states its language in `metadata.kernelspec.language` and in
/// `metadata.language_info.name`; either saying Python is enough, and a notebook that states
/// neither is taken as the Python kind (see [`notebook_code_cells`]).
fn notebook_is_python(notebook: &::serde_json::Value) -> bool {
    let metadata = notebook.get("metadata");
    let stated = |path: [&str; 2]| -> Option<String> {
        Some(
            metadata?
                .get(path[0])?
                .get(path[1])?
                .as_str()?
                .to_lowercase(),
        )
    };
    let languages = [
        stated(["kernelspec", "language"]),
        stated(["kernelspec", "name"]),
        stated(["language_info", "name"]),
    ];
    let stated: Vec<String> = languages.into_iter().flatten().collect();
    stated.is_empty() || stated.iter().any(|language| language.contains("python"))
}

/// One cell's `source`, as the notebook format spells it: one string, or the list of the
/// cell's lines.
fn cell_source(cell: &::serde_json::Value) -> Option<String> {
    match cell.get("source")? {
        ::serde_json::Value::String(source) => Some(source.clone()),
        ::serde_json::Value::Array(lines) => Some(
            lines
                .iter()
                .filter_map(::serde_json::Value::as_str)
                .collect::<Vec<&str>>()
                .concat(),
        ),
        _ => None,
    }
}

/// Start this execution's OWN run of its notebook, off the request path, and record what it
/// answered.
///
/// `StartNotebookExecution` answers once the cluster has received the request —
/// "`START_PENDING` indicates that the cluster has received the execution request but
/// execution has not begun" (`NotebookExecution$Status` doc) — so the notebook is run AFTER
/// that answer rather than inside it: this hands the run to the emulator's own runtime and
/// returns at once. The run then writes what it came to into the execution's record, which
/// is the end of its status walk ([`NotebookExecutionRecord::record_outcome`]), so the
/// execution leaves `RUNNING` when its notebook is really done.
///
/// What one run may cost is unchanged ([`run_notebook_cells`]): the pool bounds how many of
/// them this process performs at once, however many executions a caller starts, and the
/// number of executions is itself bounded ([`MAX_NOTEBOOK_EXECUTIONS`]).
pub fn start_notebook_run<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
    execution_id: String,
    location: Option<NotebookS3LocationFromInput>,
    environment: Option<HashMap<String, String>>,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    // The emulator's own in-process transport, as the request reached this service with: the
    // run reads the notebook over it after the request has been answered.
    let callback = ctx.callback().clone();
    let region = region.to_string();
    let run_state = ::engine::ServiceState::clone(&state);
    let run_id = execution_id.clone();
    // The run's own handle is recorded under the guard the run itself needs in order to
    // report, so a run that answers at once cannot clear its entry before this records it —
    // and the run stays abandonable for as long as it lasts.
    let mut store = state.lock();
    let handle = runtime.spawn(async move {
        let outcome =
            run_notebook(&callback, &region, location.as_ref(), environment.as_ref()).await;
        let now = SystemTime::now();
        let mut store = run_state.lock();
        if let Some(record) = store.notebook_execution_mut(&run_id, now) {
            record.record_outcome(outcome, now);
        }
        // This run has answered, so there is nothing left to abandon.
        store.forget_notebook_run(&run_id);
    });
    store.set_notebook_run(execution_id, handle);
}

impl ComAmazonawsEmrElasticmapreduceState {
    /// Keep the task performing the execution `id`'s run, so the run can be abandoned while
    /// it lasts. A run this replaces is abandoned rather than merely forgotten.
    fn set_notebook_run(&mut self, id: String, handle: ::tokio::task::JoinHandle<()>) {
        if let Some(previous) = self.notebook_runs.insert(id, handle) {
            previous.abort();
        }
    }

    /// Forget the run of the execution `id` without abandoning it: what a run that has
    /// already answered calls.
    fn forget_notebook_run(&mut self, id: &str) {
        self.notebook_runs.remove(id);
    }

    /// ABANDON the run of the execution `id`, if one is still under way.
    ///
    /// What `StopNotebookExecution` asks for: "`STOP_PENDING` indicates that the cluster has
    /// received a `StopNotebookExecution` request and the stop is pending"
    /// (`NotebookExecution$Status` doc), and a stopped execution ends `STOPPED` rather than
    /// in what its notebook was going to come to — so the run it was waiting for is given up
    /// on rather than left to report an end nobody is waiting for.
    pub fn abandon_notebook_run(&mut self, id: &str) {
        if let Some(handle) = self.notebook_runs.remove(id) {
            handle.abort();
        }
    }
}

/// What a notebook execution's own run answered, for the Amazon S3 notebook `location` and
/// the execution's own `environment`.
///
/// Called with NO store guard held: it follows the notebook's Amazon S3 reference over the
/// emulator's own in-process transport and then runs the notebook's cells, neither of which
/// may happen under this service's lock. The two arguments are everything of the request
/// that reaches a run — the object to read and the environment the cells see — so nothing
/// else a caller sent can steer either half.
pub async fn run_notebook(
    callback: &EmbeddedEmulator,
    region: &str,
    location: Option<&NotebookS3LocationFromInput>,
    environment: Option<&HashMap<String, String>>,
) -> NotebookOutcome {
    // "The Amazon S3 location that the notebook execution input is stored in"
    // (`StartNotebookExecutionInput$NotebookS3Location` doc): an execution that names no
    // such location names a notebook under an editor this emulator holds no notebook
    // storage for, so there is nothing to fetch and nothing is reported as having run.
    let Some(location) = location else {
        return NotebookOutcome::NotRun;
    };
    let Some(document) = notebook_object(callback, region, location).await else {
        return NotebookOutcome::NotRun;
    };
    let Some(cells) = notebook_code_cells(&document) else {
        return NotebookOutcome::NotRun;
    };
    let environment: BTreeMap<String, String> = environment
        .into_iter()
        .flatten()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    run_notebook_cells(cells, environment).await
}

/// The notebook object the Amazon S3 location `location` names, or `None` when Amazon S3
/// hands over no such object.
///
/// The read reaches nothing outside this process. The client is built from the operation's
/// own callback ([`::service_common::s3_client`]), which is the emulator's in-process
/// transport: no socket is opened, no name is resolved, and the ONE service it can reach is
/// this emulator's own Amazon S3. The location is a bucket and a key — an address inside
/// that store — so no part of the request becomes a host, a scheme, a URL or a filesystem
/// path, and an empty half of it addresses no object at all. The object is bounded by
/// [`MAX_NOTEBOOK_BYTES`] both on the length Amazon S3 reports, before any of it is held,
/// and on the bytes that arrived.
async fn notebook_object(
    callback: &EmbeddedEmulator,
    region: &str,
    location: &NotebookS3LocationFromInput,
) -> Option<Vec<u8>> {
    let bucket = location.bucket.as_deref().filter(|name| !name.is_empty())?;
    let key = location.key.as_deref().filter(|key| !key.is_empty())?;
    let read = ::service_common::s3_client::s3_client(callback, region)
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .ok()?;
    let over_the_cap = |length: usize| length > MAX_NOTEBOOK_BYTES;
    if read
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .is_some_and(over_the_cap)
    {
        return None;
    }
    let document = read.body.collect().await.ok()?.to_vec();
    (!over_the_cap(document.len())).then_some(document)
}

/// Run `cells` in the contained interpreter and report what they came to.
///
/// Three bounds hold this, beside the ones the interpreter enforces inside the run
/// ([`notebook_budget`]):
///
/// * a RUN of the process's pool ([`MAX_CONCURRENT_NOTEBOOK_RUNS`]), waited for no longer
///   than [`NOTEBOOK_PERMIT_WAIT`] — a run that finds the pool full queues for a place rather
///   than adding a thread to the process, and one that never gets a place is reported as
///   having run nothing. The permit is MOVED into the run's own thread, so it comes back when
///   that thread ends and a run that was given up on still holds its place in the pool while
///   it lasts;
/// * the run happens on a thread of its own, with the stack a run is given
///   ([`RUN_STACK_BYTES`]), because the interpreter is synchronous and CPU-bound: nothing of
///   it runs on the async executor, so the emulator stays responsive while it runs;
/// * this call waits for the answer no longer than the run's own wall clock and the grace on
///   top of it ([`NOTEBOOK_RUN_GRACE`]) — past that the run has been given up on and nothing
///   is reported as having come of it.
async fn run_notebook_cells(
    cells: Vec<String>,
    environment: BTreeMap<String, String>,
) -> NotebookOutcome {
    let budget = notebook_budget();
    let Ok(event) = ::serde_json::to_string(&::serde_json::json!({ "cells": cells })) else {
        return NotebookOutcome::NotRun;
    };
    let Ok(Ok(permit)) = ::tokio::time::timeout(
        NOTEBOOK_PERMIT_WAIT,
        ::std::sync::Arc::clone(notebook_run_permits()).acquire_owned(),
    )
    .await
    else {
        ::tracing::warn!(
            runs = MAX_CONCURRENT_NOTEBOOK_RUNS,
            "no run came free inside the wait for a place in the pool, so the notebook was \
             not run"
        );
        return NotebookOutcome::NotRun;
    };
    let (sender, receiver) = ::tokio::sync::oneshot::channel();
    let spawned = ::std::thread::Builder::new()
        .name("emr-notebook-execution".to_string())
        .stack_size(RUN_STACK_BYTES)
        .spawn(move || {
            // Held for the life of this thread, so the pool counts the run until the thread
            // is really gone — including a run the caller below stopped waiting for.
            let _permit = permit;
            let modules = vec![PackageModule {
                path: NOTEBOOK_KERNEL_MODULE.to_string(),
                source: NOTEBOOK_KERNEL_SOURCE.to_string(),
            }];
            let finished = python::run(&python::Run {
                modules: &modules,
                handler: NOTEBOOK_KERNEL_HANDLER,
                environment: &environment,
                event: Some(&event),
                request_id: "",
                invoked_function_arn: "",
                client_context: None,
                tenant_id: None,
                budget,
                // The cells reach no other service of this emulator: a notebook
                // execution is given no seam to dispatch an AWS call on.
                aws: None,
            });
            let _ = sender.send(finished);
        });
    if spawned.is_err() {
        return NotebookOutcome::NotRun;
    }
    let waited = ::tokio::time::timeout(
        budget.wall_clock.saturating_add(NOTEBOOK_RUN_GRACE),
        receiver,
    )
    .await;
    match waited {
        Ok(Ok(finished)) => notebook_outcome(&finished),
        // The run was given up on, or its thread ended without reporting: nothing is known
        // about what the notebook came to, so nothing is reported as having run.
        Ok(Err(_)) | Err(_) => NotebookOutcome::NotRun,
    }
}

/// What the finished run answered, as the execution's own end.
fn notebook_outcome(finished: &Finished) -> NotebookOutcome {
    match &finished.completion {
        // The kernel returns how many cells it ran, which is the answer this reads: a run
        // that returned ran every cell of the notebook without raising.
        Completion::Returned(answer) => NotebookOutcome::Finished {
            cells: ::serde_json::from_str::<::serde_json::Value>(answer)
                .ok()
                .and_then(|document| {
                    document
                        .get("cells")
                        .and_then(::serde_json::Value::as_u64)
                })
                .and_then(|cells| usize::try_from(cells).ok())
                .unwrap_or_default(),
        },
        // The notebook's own exception, as the engine raised it: the type it is and the
        // message it carries, which is the reason the execution failed.
        Completion::Raised {
            error_type,
            error_message,
            ..
        } => NotebookOutcome::Failed {
            reason: notebook_failure_reason(error_type, error_message),
        },
        // A bound the run ran past, named by the bound it was.
        Completion::Stopped(stop) => NotebookOutcome::Failed {
            reason: notebook_bound_reason(*stop),
        },
        // The interpreter could not be built or driven, so no cell of the notebook ran.
        Completion::Internal(_) | Completion::Streamed { .. } => NotebookOutcome::NotRun,
    }
}

/// `NotebookExecution$LastStateChangeReason` for a notebook whose own cells raised: the
/// exception as the engine reports it.
///
/// Both parts come from the RUN — the exception's type and the message it carries — so the
/// reason a caller reads is the notebook's own failure and nothing this emulator worded.
fn notebook_failure_reason(error_type: &str, error_message: &str) -> String {
    if error_message.trim().is_empty() {
        return error_type.to_string();
    }
    format!("{error_type}: {error_message}")
}

/// `NotebookExecution$LastStateChangeReason` for a notebook the run STOPPED at one of its
/// bounds: which bound it was, in the notebook's own terms.
///
/// The bound is the run's, so the sentence states it as something the notebook did rather
/// than as something the notebook's code said: there is no exception to report, because the
/// cells were stopped rather than raised.
fn notebook_bound_reason(stop: Stop) -> String {
    let bound = match stop {
        Stop::Deadline => "ran longer than an execution may run for",
        Stop::Steps => "did more work than an execution may do",
        Stop::Memory => "needed more memory than an execution may use",
        Stop::Response => "produced more output than an execution may produce",
        Stop::HostCeiling => "took more of this emulator's host time than an execution may take",
    };
    format!("The notebook was stopped: it {bound}.")
}
