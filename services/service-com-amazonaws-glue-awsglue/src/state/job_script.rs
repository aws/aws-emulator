// SPDX-License-Identifier: CC0-1.0

//! RUNNING the script of a Glue job run: where the script comes from, what it runs in, and
//! what the run it belongs to comes to rest in.
//!
//! "Starts a job run using a job definition" (`StartJobRun`), and a job definition names
//! the script it runs: `JobCommand$ScriptLocation` is "the Amazon Simple Storage Service
//! (Amazon S3) path to a script that executes a job", and `StartJobRun$Arguments` are "the
//! job arguments associated with this run … that your own job-execution script consumes".
//! So a run of a job IS one execution of that script with those arguments, and the state
//! the run comes to rest in is whatever that execution came to: `SUCCEEDED` for a script
//! that exited cleanly, and the modelled failure state carrying the SCRIPT'S own error for
//! one that raised.
//!
//! That is what this module does, and nothing about it is synthesised:
//!
//! * the script is READ from the emulator's own Amazon S3, over the framework's in-process
//!   callback ([`smithy_server_runtime::OperationContext::callback`]) — the same transport a
//!   client's own S3 request takes, so the bytes that run are the bytes that were uploaded;
//! * it is EXECUTED by the contained Python interpreter this workspace already embeds (the
//!   Lambda service crate's `state::execution::python`), with the AWS SDK modules that
//!   runtime supplies on the import path — which is what a Glue job script finds in its own
//!   environment — so a script's `boto3` call reaches this same emulator and its effects
//!   (an object written to a bucket, a catalog write) are really made. No second interpreter
//!   is stood up here, and no part of a script is interpreted by this crate;
//! * what the run REPORTS is what that execution answered: the terminal `JobRunState` is
//!   decided by the outcome ([`JobRunOutcome`]), `JobRun$ErrorMessage` of a failed run is the
//!   exception the script raised, and a run that exited cleanly carries no error message at
//!   all.
//!
//! # Where the script runs from, and when
//!
//! The execution is performed where a run is BEGUN — by `StartJobRun`, by a trigger's
//! actions, or by a trigger whose batch window the clock ran out on — with no store guard
//! held, exactly as a column statistics task run reads its table's data
//! ([`super::compute_column_statistics`]). A run whose script has not been performed yet is
//! marked [`JobRunEntry::awaiting_script`], so an operation that reads a job's runs performs
//! what that run was already owed rather than reporting a run nothing is running: the
//! execution was owed the instant the run began, whoever began it.
//!
//! WHAT the run then REPORTS is the clock's, not this module's: the state the execution came
//! to is owed one span after the run has been [`JOB_RUN_STATE_RUNNING`], and never before one
//! span after the execution itself answered ([`JobRunEntry::record_execution`]) — so the run
//! passes through `RUNNING` for as long as its script takes, and a client polling it observes
//! every state it goes through.
//!
//! # Bounds
//!
//! A script is caller-supplied code, so every bound is this emulator's and none of them is
//! reachable from the script: how many scripts run at once
//! ([`MAX_CONCURRENT_JOB_SCRIPT_RUNS`], with a bounded wait for a permit), how many one call
//! performs ([`MAX_JOB_SCRIPT_RUNS_PER_CALL`]) and for how long
//! ([`JOB_SCRIPT_CALL_CEILING`]), how long one script's own run may last
//! ([`JOB_SCRIPT_RUN_CEILING`], which also clamps the job's own `Timeout`), how much of THIS
//! HOST's time one run may take ([`JOB_SCRIPT_HOST_CEILING`], which the engine stops the run
//! on) and how long the call waits for the thread it runs on ([`JOB_SCRIPT_HOST_WAIT`]), how
//! large the script may be ([`MAX_JOB_SCRIPT_BYTES`], enforced by the ranged read), and the
//! step ([`MAX_JOB_SCRIPT_STEPS`]), memory, recursion, log and answer ceilings the interpreter
//! is given ([`job_script_budget`]).
//!
//! # What decides the state a run comes to rest in
//!
//! What the run REPORTS is decided by what the EXECUTION did and never by how fast this host
//! happened to be running it: the span a script's own run may last is read against a clock
//! the RUN drives ([`python::Clock::driven`], one tick [`JOB_SCRIPT_CLOCK_TICK`] per traced
//! event it delivers) and spent by the waits the script itself asks for, so a script that ran
//! to its end is reported on what it came to however long this host took over it, a script
//! that WAITS past that span is the one reported as terminated for taking too long, and a
//! script that computes without end is answered by its step budget.
//!
//! THIS HOST is bounded separately, and by a bound that reports no outcome of the script: the
//! same clock states the host time one run may take ([`python::Clock::within`]), which the
//! engine reads from this host's own clock at every traced event and from its own watch over
//! the run — so a script spending events, and one sitting in a long native call that delivers
//! none, are both stopped. A run stopped there is one this emulator gave up on, reported in
//! the state a service error puts a run in ([`job_run_script_host_ceiling_reached`]), as is
//! one whose thread had still not answered when the call stopped waiting for it
//! ([`job_run_script_unanswered`]).
//!
//! # What a script can and cannot reach
//!
//! NO PROCESS IS SPAWNED and no shell is involved: the script is compiled and executed
//! inside this process by the embedded interpreter, on a thread of its own, and the only
//! things it can reach are the ones that engine registers for it — the frozen standard
//! library, the AWS SDK modules whose calls travel over this emulator's own in-process
//! callback, and the operating-system modules this workspace supplies in place of the host's.
//! No host path is resolved, no host process or socket is reachable, no host library is
//! loaded, and the environment the script sees is empty rather than this process's. That
//! containment — the native modules that are never registered, and the stop no guest call can
//! clear — is the engine module's, which is why this module stands none of it up again.

use ::std::collections::BTreeMap;
use ::std::time::{Duration, SystemTime};

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::aws_api::CallerIdentity;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::execution_role_session::execution_role_identity;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::python;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    function_sdk_access, AwsAccess, INIT_PHASE_LIMIT,
};

use super::*;

/// How large a job script this emulator reads.
///
/// CHOSEN, not an AWS limit: the script is an object a caller uploaded, so what one run
/// takes into this process is bounded here rather than by the object's own size. The bound
/// is enforced by the RANGED read, so nothing larger is ever buffered.
pub const MAX_JOB_SCRIPT_BYTES: usize = 1024 * 1024;

/// The longest ONE run of a job script may last in the RUN'S OWN time, whatever the job's
/// `Timeout` says.
///
/// CHOSEN, not an AWS duration: `Timeout` is stated in minutes and may be as long as seven
/// days, and the execution happens inside the call that begins the run — so this is the span
/// this emulator gives a script, and the job's own `Timeout` is clamped DOWN to it before it
/// bounds anything ([`job_script_budget`]). Because the shortest `Timeout` the model admits
/// is one minute (`Timeout`'s `@range(min: 1)`), this ceiling is what binds every run today,
/// and a script that runs past it is reported in the state `Job$Timeout` documents for a run
/// that was terminated for taking too long.
///
/// It is a span of the RUN's own time rather than of this host's: it is read against a clock
/// the run itself drives ([`JOB_SCRIPT_CLOCK_TICK`]) and spent by the waits the script asks
/// for, so what a run comes to rest in is the same on an idle host and on a loaded one, and
/// the host's own speed is not what decides whether a script was terminated. What the run may
/// take of THIS host is bounded beside it ([`JOB_SCRIPT_HOST_CEILING`]).
pub const JOB_SCRIPT_RUN_CEILING: Duration = Duration::from_secs(5);

/// How far ONE reading of a job script run's clock moves it.
///
/// The span a script's run may last ([`JOB_SCRIPT_RUN_CEILING`]) is spent by what the RUN
/// does, and a traced event is one of those: the engine reads the run's clock once per event
/// the script delivers and once per sample the host's watch takes over it. The tick is sized
/// so that the whole step budget ([`MAX_JOB_SCRIPT_STEPS`]) spends well under that span —
/// which is what leaves a script that computes without end to be answered by the step budget,
/// a count of its OWN work, and leaves the span itself for the waits a script asks for.
pub const JOB_SCRIPT_CLOCK_TICK: Duration = Duration::from_nanos(100);

/// How much of THIS HOST's own time ONE run of a job script may take, past which the engine
/// stops it whatever its own clock reads.
///
/// CHOSEN: the span above is the RUN's own time, so it bounds no CPU of this host — a script
/// spending traced events, or sitting in one long native call that delivers none, would hold
/// a thread of this process for as long as the host took over it. This is the bound that
/// stops it: the engine reads it from this host's own clock at every traced event and from the
/// host's own watch over the run ([`python::Clock::within`]), so a run of caller-supplied code
/// is bounded on this host however its own time is read.
///
/// It is generous beside the span the engine's phases take in the run's own time
/// ([`INIT_PHASE_LIMIT`] standing the execution environment up, then
/// [`JOB_SCRIPT_RUN_CEILING`] of script) — this host may need several times as long of its own
/// for the same work when it is loaded, and a script that FINISHED must be reported on what it
/// came to rather than on the host's load — and it is many times the host time the whole step
/// budget spends ([`MAX_JOB_SCRIPT_STEPS`]), so a script that computes without end is still
/// answered by that budget rather than by this ceiling.
///
/// A run this ceiling stops is one this emulator GAVE UP ON: it is reported in the state a run
/// that ended on a service error reports, carrying [`job_run_script_host_ceiling_reached`],
/// never as one terminated for taking too long — nothing of the script's own span ran out.
pub const JOB_SCRIPT_HOST_CEILING: Duration = INIT_PHASE_LIMIT
    .saturating_add(JOB_SCRIPT_RUN_CEILING)
    .saturating_mul(HOST_CEILING_OVER_THE_ENGINE_SPAN);

/// How many times the engine's own spans the host ceiling allows for
/// ([`JOB_SCRIPT_HOST_CEILING`]).
const HOST_CEILING_OVER_THE_ENGINE_SPAN: u32 = 3;

/// How long the call that performs ONE execution waits for the thread that execution runs on.
///
/// The host ceiling above plus the grace the engine's own stop needs to land: the stop is
/// latched by the host and RAISED at the next traced event the interpreter delivers, so a
/// thread the engine has stopped answers a moment after the ceiling rather than at it. A
/// thread that has still not answered by then is given up on and the run reported as one this
/// emulator has no outcome for ([`job_run_script_unanswered`]) — never as one terminated for
/// taking too long, because nothing of the script's own span ran out. The thread keeps its
/// permit until it ends, which the ceiling above is what bounds.
pub const JOB_SCRIPT_HOST_WAIT: Duration =
    JOB_SCRIPT_HOST_CEILING.saturating_add(JOB_SCRIPT_STOP_GRACE);

/// How long the engine's own stop is given to land, once the host ceiling has latched it.
///
/// CHOSEN: the stop is raised at the next traced event, which a running script delivers at
/// once; this is what the call allows for it before it stops waiting.
pub const JOB_SCRIPT_STOP_GRACE: Duration = Duration::from_secs(5);

/// How many job scripts run at once in this process.
///
/// CHOSEN: a script runs per job run, so without this the bounds above would be multiplied
/// by the number of runs an account starts. Two scripts run at a time and the rest wait for
/// a permit.
pub const MAX_CONCURRENT_JOB_SCRIPT_RUNS: usize = 2;

/// How long a run waits for one of [`MAX_CONCURRENT_JOB_SCRIPT_RUNS`], at most.
///
/// CHOSEN: the wait is bounded, so the call that began the run answers either way, and it is
/// narrowed further by what the CALL has left of [`JOB_SCRIPT_CALL_CEILING`] — so waiting for
/// an interpreter cannot take a request past that span. A run that waited the whole wait has
/// not been performed at all: it stays [`JobRunEntry::awaiting_script`], so a later call
/// performs it rather than this one reporting an outcome nothing produced.
pub const JOB_SCRIPT_PERMIT_WAIT: Duration = Duration::from_secs(5);

/// The permits [`MAX_CONCURRENT_JOB_SCRIPT_RUNS`] hands out, for the whole process: the
/// bound is on how many scripts this emulator runs at once, whichever job or caller asked.
///
/// Held as an owned permit MOVED INTO the thread that runs the script, so it is given back
/// when that thread really ends — including a run the caller has already given up waiting
/// for, which would otherwise take this bound past its count while its thread was still
/// going.
static JOB_SCRIPT_RUN_PERMITS: ::std::sync::LazyLock<::std::sync::Arc<::tokio::sync::Semaphore>> =
    ::std::sync::LazyLock::new(|| {
        ::std::sync::Arc::new(::tokio::sync::Semaphore::new(MAX_CONCURRENT_JOB_SCRIPT_RUNS))
    });

/// How many job scripts ONE call performs.
///
/// CHOSEN: a call that finds several runs awaiting their script performs this many and
/// leaves the rest for the next call, so no single request can be made to run an unbounded
/// number of scripts.
pub const MAX_JOB_SCRIPT_RUNS_PER_CALL: usize = 8;

/// How long ONE call spends performing owed scripts before it leaves the rest to the next
/// call.
///
/// CHOSEN: the ceiling on what a REQUEST costs, beside the ceiling on what one script's own
/// run costs ([`JOB_SCRIPT_RUN_CEILING`]). A further script is STARTED only while the call
/// has not already spent this span, the wait for an interpreter is narrowed to what is left
/// of it ([`JOB_SCRIPT_PERMIT_WAIT`]), and no script in flight is ever cancelled — so a run
/// is never left claimed by an execution nobody is waiting for, and the longest a call can be
/// held is this span plus what the LAST execution it began is waited for
/// ([`JOB_SCRIPT_HOST_WAIT`]) rather than the product of that wait and
/// [`MAX_JOB_SCRIPT_RUNS_PER_CALL`].
pub const JOB_SCRIPT_CALL_CEILING: Duration = Duration::from_secs(5);

/// How many jobs ONE call performs owed scripts for, for the same reason.
pub const MAX_JOBS_PER_SCRIPT_CALL: usize = 8;

/// The stack a script's thread is given: well above the interpreter's clamped recursion
/// limit, so deep recursion in a script ends that run with the interpreter's own
/// `RecursionError` rather than touching the thread's guard page.
const JOB_SCRIPT_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How many traced events one script may execute.
///
/// CHOSEN: large enough for any ETL script a client runs here — it is several times the
/// events the scripts of this crate's own suites deliver, the AWS SDK calls they make
/// included — and small enough that a loop with no exit ends in a small part of the host time
/// one run may take ([`JOB_SCRIPT_HOST_CEILING`]), so such a loop is answered by THIS budget
/// rather than by that ceiling. It is a count of the script's OWN work, so it is the same
/// count on an idle host and on a loaded one, which is what makes the answer to a script that
/// computes without end the same on both. The span the run's clock gives it allows for this
/// whole budget ([`JOB_SCRIPT_CLOCK_TICK`]), so a script that spends the budget is reported on
/// the budget it ran past rather than on the clock.
pub const MAX_JOB_SCRIPT_STEPS: u64 = 1_000_000;

/// How much memory one script may add to this process. CHOSEN, for the same reason.
const MAX_JOB_SCRIPT_MEMORY_BYTES: u64 = 512 * 1024 * 1024;

/// How deep one script may recurse. The interpreter's own default is below what the frozen
/// standard library's import chain needs, so this is the limit a run is given; the call that
/// would raise it is taken away by the engine before any script code runs.
const MAX_JOB_SCRIPT_RECURSION: usize = 2_000;

/// How much of a script's output is kept, and how large a value it may answer with.
///
/// Neither reaches the wire: a Glue job run reports no script output, so these only bound
/// what one run holds in memory.
const MAX_JOB_SCRIPT_LOG_BYTES: usize = 64 * 1024;
/// See [`MAX_JOB_SCRIPT_LOG_BYTES`].
const MAX_JOB_SCRIPT_ANSWER_BYTES: usize = 64 * 1024;

/// The job argument naming the LANGUAGE a job's script is written in: "`--job-language` …
/// the script programming language. This value must be either `scala` or `python`. If this
/// parameter is not present, the default is `python`." (AWS Glue's special job parameters.)
pub const JOB_LANGUAGE_ARGUMENT: &str = "--job-language";

/// The `--job-language` value this emulator runs a script for: Python, which is also the
/// default when a job names no language. A job whose script is in any other language is one
/// no engine here runs, and its run says exactly that rather than reporting an outcome.
pub const PYTHON_JOB_LANGUAGE: &str = "python";

/// `JobRun$ErrorMessage` of a run whose job names no script to run.
///
/// Whole fixed text: `JobCommand$ScriptLocation` is the Amazon S3 path of the script a run
/// executes, so a command that carries none names nothing for the run to execute. It states
/// what was missing and claims nothing about work nobody ran.
pub const JOB_RUN_NO_SCRIPT_LOCATION: &str =
    "The job's Command names no ScriptLocation, so the job run had no script to execute.";

/// The fixed words [`job_run_script_unreadable`] leads with, before the location of the
/// script it is about.
pub const JOB_RUN_SCRIPT_UNREADABLE_LEAD: &str = "Unable to download the job script from";

/// `JobRun$ErrorMessage` of a run whose script could not be read from the location the job
/// names.
///
/// The LOCATION comes from the stored job definition and the detail from what the object
/// store answered, so the message says which script could not be read and why rather than
/// repeating one run's values.
#[must_use]
pub fn job_run_script_unreadable(location: &str, detail: &str) -> String {
    format!("{JOB_RUN_SCRIPT_UNREADABLE_LEAD} {location}: {detail}")
}

/// The fixed text [`job_run_script_not_accessible`] states, before the script it is about.
///
/// RECORDED: the service's own words for a job run whose script it could not fetch.
pub const JOB_RUN_SCRIPT_NOT_ACCESSIBLE_LEAD: &str = "Script file doesn't exist";

/// `JobRun$ErrorMessage` of a run whose execution role may not READ the object the job's
/// `JobCommand$ScriptLocation` names.
///
/// RECORDED template: the fixed text plus ONE variable part, the script the stored job
/// definition points at — so the message names the object the run could not fetch and
/// nothing of one run's values.
#[must_use]
pub fn job_run_script_not_accessible(location: &str) -> String {
    format!("{JOB_RUN_SCRIPT_NOT_ACCESSIBLE_LEAD}: {location}")
}

/// The name the session a job run's calls are signed with is reported under, for a client of
/// the credential store that asks where a credential came from. It names the service and the
/// kind of session, never a value of one run.
///
/// Whether the policies in force on that role admit the script read is NOT decided in this
/// crate: the script is fetched AS the role ([`read_script`]), and the object store answers
/// that question the way it answers it for every other signed request — against the role's
/// own policies and the bucket's.
const JOB_RUN_CREDENTIAL_SOURCE: &str = "glue-job-run-execution-role";

/// `JobRun$ErrorMessage` of a run whose script is in a language no engine of this emulator
/// runs.
///
/// Glue runs a job's script in the language its `--job-language` argument names, and this
/// emulator embeds an interpreter for [`PYTHON_JOB_LANGUAGE`] only — so a run of a job in
/// another language reports that nothing ran it, rather than an outcome no engine produced.
#[must_use]
pub fn job_run_language_not_runnable(language: &str) -> String {
    format!(
        "The job's {JOB_LANGUAGE_ARGUMENT} is {language}, and this emulator runs a job script \
         written in {PYTHON_JOB_LANGUAGE} only, so the job run's script was not executed."
    )
}

/// `JobRun$ErrorMessage` of a run whose script ran past the span this emulator gives one run
/// of a job script ([`JOB_SCRIPT_RUN_CEILING`]).
///
/// The run reaches the state `JobRun$Timeout` documents — "terminated and enters `TIMEOUT`
/// status" — and the message names the ceiling that stopped it rather than the job's own
/// timeout, which is longer and was not reached.
#[must_use]
pub fn job_run_script_ceiling_reached() -> String {
    format!(
        "The job script was stopped after {} seconds, the longest this emulator runs a job \
         script for.",
        JOB_SCRIPT_RUN_CEILING.as_secs()
    )
}

/// `JobRun$ErrorMessage` of a run whose script's thread had still not answered when the call
/// that performs it gave up waiting ([`JOB_SCRIPT_HOST_WAIT`]).
///
/// Nothing of the script was stopped and no execution answered, so the run reports the state
/// a run that ended on a SERVICE error reports ([`JOB_RUN_STATE_ERROR`]) and says that this
/// emulator has no outcome for it — rather than claiming the script was terminated for taking
/// too long, which is an outcome no execution produced.
#[must_use]
pub fn job_run_script_unanswered() -> String {
    format!(
        "The job script's execution did not answer within {} seconds, so this emulator has no \
         outcome to report for the job run.",
        JOB_SCRIPT_HOST_WAIT.as_secs()
    )
}

/// `JobRun$ErrorMessage` of a run the engine stopped because it had taken the host time one
/// run of a job script may take ([`JOB_SCRIPT_HOST_CEILING`]).
///
/// The script was stopped by this emulator rather than by anything of the run's own, so the
/// message says so and the run ends in the state a service error puts it in: the script's own
/// span had not run out, and what it would have come to is not known.
#[must_use]
pub fn job_run_script_host_ceiling_reached() -> String {
    format!(
        "The job script was stopped after taking {} seconds of this emulator's own time, so \
         this emulator has no outcome to report for the job run.",
        JOB_SCRIPT_HOST_CEILING.as_secs()
    )
}

/// `JobRun$ErrorMessage` of a run whose script ran past one of the interpreter's own
/// ceilings — its step budget, its memory ceiling or the size of the value it answered.
#[must_use]
pub fn job_run_script_bound_reached(bound: &str) -> String {
    format!("The job script was stopped because it ran past {bound}.")
}

/// What ONE execution of a job's script came to, which is what the state the clock owes the
/// run reports ([`JobRunEntry::record_execution`]).
#[derive(Debug, Clone)]
pub enum JobRunOutcome {
    /// The script ran and exited cleanly, so the run comes to rest in
    /// [`JOB_RUN_STATE_SUCCEEDED`] carrying no error message.
    Succeeded,
    /// The script raised, or there was no script to run: the run comes to rest in
    /// [`JOB_RUN_STATE_FAILED`] carrying this reason — the exception the script itself
    /// raised, where a script ran.
    Failed(String),
    /// The script ran past the run's own `Timeout`, which is the state that member
    /// documents ([`JOB_RUN_STATE_TIMEOUT`]).
    TimedOut(String),
    /// The interpreter itself could not be built or driven. Not the script's failure, so it
    /// is the state a run that ended on a service error reports ([`JOB_RUN_STATE_ERROR`]).
    EndedInError(String),
}

impl JobRunOutcome {
    /// The `JobRunState` a run that came to this comes to rest in.
    #[must_use]
    pub fn job_run_state(&self) -> &'static str {
        match self {
            Self::Succeeded => JOB_RUN_STATE_SUCCEEDED,
            Self::Failed(_) => JOB_RUN_STATE_FAILED,
            Self::TimedOut(_) => JOB_RUN_STATE_TIMEOUT,
            Self::EndedInError(_) => JOB_RUN_STATE_ERROR,
        }
    }

    /// `JobRun$ErrorMessage` the run carries once it has come to rest: "the error message
    /// associated with this job run", which a run that exited cleanly has none of.
    #[must_use]
    pub fn error_message(&self) -> Option<String> {
        match self {
            Self::Succeeded => None,
            Self::Failed(message) | Self::TimedOut(message) | Self::EndedInError(message) => {
                Some(message.clone())
            }
        }
    }
}

/// Everything one execution of a job's script needs, copied out of the store under its
/// guard so the execution itself holds none.
#[derive(Debug, Clone)]
pub struct JobScriptPlan {
    /// The job definition the run is a run of.
    pub job_name: String,
    /// The run whose script this is.
    pub run_id: String,
    /// `JobCommand$ScriptLocation`: the Amazon S3 path of the script to run.
    pub script_location: Option<String>,
    /// The run's own arguments — the job's `DefaultArguments` with the start's `Arguments`
    /// over them — which the script reads off its command line.
    pub arguments: BTreeMap<String, String>,
    /// `JobRun$Timeout`, in minutes, as the run carries it.
    pub timeout_minutes: i32,
    /// The ARN of the role the run's own calls are made as: "the name or Amazon Resource
    /// Name (ARN) of the IAM role associated with this job" (`Job$Role`), resolved by the
    /// crate's one reading of that member ([`glue_role_reference`]).
    pub role_arn: String,
}

impl JobScriptPlan {
    /// The plan for the run `run_id` of `job`, or `None` when the job holds no such run.
    #[must_use]
    pub fn of(job: &JobEntry, run_id: &str) -> Option<Self> {
        let run = job.run(run_id)?;
        Some(Self {
            job_name: job.name.clone(),
            run_id: run.id.clone(),
            script_location: job.command.script_location.clone(),
            arguments: run
                .arguments
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            timeout_minutes: run.timeout,
            role_arn: glue_role_reference(&job.role).arn,
        })
    }

    /// The `--job-language` the job's arguments name, else the documented default
    /// ([`PYTHON_JOB_LANGUAGE`]), folded to lower case the way the values are written.
    #[must_use]
    pub fn job_language(&self) -> String {
        self.arguments
            .get(JOB_LANGUAGE_ARGUMENT)
            .map(|language| language.trim().to_ascii_lowercase())
            .filter(|language| !language.is_empty())
            .unwrap_or_else(|| PYTHON_JOB_LANGUAGE.to_string())
    }

    /// The command line the script reads its arguments off.
    ///
    /// A job argument is named by the `--name` a script resolves it under, so the line is
    /// the script's own path followed by each argument as `--name value` — in name order, so
    /// one run's line does not depend on a map's iteration order. A stored name that does
    /// not already carry the marker is given it, because that is the form a script resolves.
    #[must_use]
    pub fn command_line_arguments(&self) -> Vec<String> {
        let mut line = Vec::with_capacity(self.arguments.len() * 2);
        for (name, value) in &self.arguments {
            let flagged = if name.starts_with("--") {
                name.clone()
            } else {
                format!("--{name}")
            };
            line.push(flagged);
            line.push(value.clone());
        }
        line
    }
}

/// What one script run may spend: the job's own `Timeout` clamped DOWN to this emulator's
/// own ceiling ([`JOB_SCRIPT_RUN_CEILING`]), beside the step, memory, recursion, log and
/// answer ceilings every run here is given.
///
/// The clamp is what keeps a stored `Timeout` — up to the seven days `Job$Timeout` admits —
/// from setting the span one run of a script is given. The span is the RUN's own time, read
/// against the clock [`run_script`] hands the engine, so no stored value sets the lifetime of
/// a thread of this process either: how long THIS host is held is the backstop's
/// ([`JOB_SCRIPT_HOST_WAIT`]).
#[must_use]
pub fn job_script_budget(timeout_minutes: i32) -> python::Budget {
    python::Budget {
        wall_clock: configured_timeout(timeout_minutes).min(JOB_SCRIPT_RUN_CEILING),
        steps: MAX_JOB_SCRIPT_STEPS,
        memory_bytes: MAX_JOB_SCRIPT_MEMORY_BYTES,
        log_bytes: MAX_JOB_SCRIPT_LOG_BYTES,
        response_bytes: MAX_JOB_SCRIPT_ANSWER_BYTES,
        recursion_limit: MAX_JOB_SCRIPT_RECURSION,
    }
}

/// The run's own `Timeout` as a duration: "The `JobRun` timeout in minutes", through the one
/// conversion this service holds a timeout in minutes with
/// ([`timeout_minutes_as_duration`]), inside the range the model allows for it (`Timeout`'s
/// `@range(min: 1)` and the documented ceiling of [`MAX_TIMEOUT_MINUTES`]) so a stored value
/// outside that range bounds nothing.
fn configured_timeout(timeout_minutes: i32) -> Duration {
    timeout_minutes_as_duration(timeout_minutes, MAX_TIMEOUT_MINUTES)
}

/// Perform the script of every run of `job_names` that is still awaiting one, and record
/// what each execution came to.
///
/// This is the ONE place a job run's script is run from, so a run begun by `StartJobRun`, by
/// a trigger's actions and by a trigger whose batch window ran out are all executed the same
/// way. Awaited with NO store guard held: the plan is copied out under the guard, the script
/// runs without it, and the outcome is recorded under it again.
///
/// What the run REPORTS while and after that is the clock's: the state the execution came to
/// is owed one span after the run has been [`JOB_RUN_STATE_RUNNING`], and never before one
/// span after the execution answered ([`JobRunEntry::record_execution`]) — so a client
/// observes the run starting, then running, then at rest, whichever call performed which
/// part.
///
/// NOTHING IS SPAWNED: each script is compiled and executed inside this process by the
/// embedded interpreter, contained and bounded as the module docs set out. What ONE call
/// costs is bounded twice over — at most [`MAX_JOB_SCRIPT_RUNS_PER_CALL`] scripts, and a
/// further one started only while the call has not already spent
/// [`JOB_SCRIPT_CALL_CEILING`] — so a slow or looping script cannot hold the request that
/// called this open beyond that span and what the last execution it began is waited for
/// ([`JOB_SCRIPT_HOST_WAIT`]), and the runs it did not reach stay owed rather than being
/// reported as anything.
///
/// A run that FAILED and still has retry budget owes the service a further attempt
/// (`Job$MaxRetries`), which is recorded once that run has come to rest and performed by
/// this same path — the service runs a retry, not the caller — so a chain of attempts
/// advances one attempt per span the clock owes each of them rather than one call running
/// the whole chain.
pub async fn perform_owed_job_runs<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    job_names: &[String],
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let began = ::std::time::Instant::now();
    let mut performed = 0usize;
    for job_name in job_names.iter().take(MAX_JOBS_PER_SCRIPT_CALL) {
        while performed < MAX_JOB_SCRIPT_RUNS_PER_CALL
            && began.elapsed() < JOB_SCRIPT_CALL_CEILING
        {
            // What the next owed execution is, resolved under the guard; the guard is
            // released before anything is run.
            let plan = {
                let mut store = state.lock();
                let Some(job) = store.job_mut(region, job_name) else {
                    break;
                };
                // Everything the clock already owed this job's runs is materialised first,
                // so a run that has already come to rest is not performed again — and the
                // attempt the retry policy owes a run that came to rest in the failure
                // state is recorded here, so it is performed like any other owed run.
                job.settle_due_runs(SystemTime::now());
                job.record_owed_attempts();
                // The execution is CLAIMED under the guard, so two calls that find the same
                // run owed its script cannot both run it.
                let Some(run_id) = job.claim_run_awaiting_script() else {
                    break;
                };
                match JobScriptPlan::of(job, &run_id) {
                    Some(plan) => plan,
                    // Nothing can be run without a plan, so the claim goes straight back:
                    // a run left claimed by an execution nobody performs would report the
                    // state it is in while its script runs for ever.
                    None => {
                        job.release_script_claim(&run_id);
                        break;
                    }
                }
            };

            let Some(outcome) = perform_job_script(
                engine,
                ctx,
                region,
                &plan,
                JOB_SCRIPT_CALL_CEILING.saturating_sub(began.elapsed()),
            )
            .await
            else {
                // Nothing ran: the claim goes back, so a later call performs the run rather
                // than this one reporting an outcome no execution produced.
                let mut store = state.lock();
                if let Some(job) = store.job_mut(region, job_name) {
                    job.release_script_claim(&plan.run_id);
                }
                break;
            };
            performed += 1;

            let mut store = state.lock();
            let Some(job) = store.job_mut(region, job_name) else {
                break;
            };
            if let Some(run) = job.run_mut(&plan.run_id) {
                run.record_execution(&outcome, SystemTime::now());
            }
            ::tracing::info!(
                job = %plan.job_name,
                run = %plan.run_id,
                state = %outcome.job_run_state(),
                "ran a Glue job run's script"
            );
        }
    }

    // A run that has COME TO REST is an event this service publishes: the state changes the
    // clock materialised above are recorded and put on the account's default EventBridge bus
    // here, with the store's guard released, so the call that brings a run to rest is the
    // call that publishes it ([`super::job_run_events`]).
    // A run that has COME TO REST is an event this service publishes: the state changes the
    // clock materialised above are recorded here, with the store's guard released, and the
    // publication — and every state the clock still owes these runs — is handed to the clock
    // pump, so no request waits on another service for it ([`super::job_run_events`]).
    record_and_publish_job_run_state_changes(
        ctx.callback(),
        &state,
        region,
        // The jobs this call performed owed work for, bounded the same way that work is.
        &job_names[..job_names.len().min(MAX_JOBS_PER_SCRIPT_CALL)],
    );
}

/// Run ONE job script and answer what it came to, or `None` when nothing ran it.
///
/// Every answer is grounded in what really happened: the object store's own refusal for a
/// script it does not serve, the exception the script raised, or the bound the interpreter
/// stopped it at.
///
/// `left_of_the_call` is what the CALL still has of [`JOB_SCRIPT_CALL_CEILING`], which caps
/// the wait for a permit: waiting for an interpreter cannot then take the call past its own
/// span, so the longest a request is held is that span plus what the execution it began is
/// waited for ([`JOB_SCRIPT_HOST_WAIT`]).
async fn perform_job_script<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    plan: &JobScriptPlan,
    left_of_the_call: Duration,
) -> Option<JobRunOutcome> {
    // A job run is performed BY Glue acting as the job's `Role` ("the name or Amazon
    // Resource Name (ARN) of the IAM role associated with this job", `Job$Role`), so the
    // run is admitted only once this emulator's own IAM has been asked whether Glue can
    // assume that role at all: a role whose trust policy does not admit the Glue service
    // principal backs no run, and the run fails before anything of the job is fetched or
    // executed. Only what IAM ANSWERED fails the run (see
    // [`super::glue_role_is_unassumable`]), so a role nothing can be shown about — including
    // an emulator with no IAM to ask — is admitted. Awaited with no store guard held.
    if glue_role_is_unassumable(ctx, region, &plan.role_arn).await {
        ::tracing::debug!(
            job = %plan.job_name,
            run = %plan.run_id,
            "a Glue job run's execution role cannot be assumed by Glue, so nothing was run"
        );
        return Some(JobRunOutcome::Failed(messages::job_run_role_not_assumable(
            &plan.role_arn,
        )));
    }
    let language = plan.job_language();
    if language != PYTHON_JOB_LANGUAGE {
        return Some(JobRunOutcome::Failed(job_run_language_not_runnable(
            &language,
        )));
    }
    let Some(location) = plan
        .script_location
        .as_deref()
        .map(str::trim)
        .filter(|location| !location.is_empty())
    else {
        return Some(JobRunOutcome::Failed(
            JOB_RUN_NO_SCRIPT_LOCATION.to_string(),
        ));
    };
    let Some((bucket, key)) = split_s3_location(location) else {
        return Some(JobRunOutcome::Failed(job_run_script_unreadable(
            location,
            "the location does not name an Amazon S3 object",
        )));
    };
    if key.is_empty() || key.ends_with('/') {
        return Some(JobRunOutcome::Failed(job_run_script_unreadable(
            location,
            "the location does not name an Amazon S3 object",
        )));
    }
    // The script is FETCHED by Glue acting as that same role, so the read is made with a
    // SESSION of it ([`execution_role_identity`], the one mint this workspace has for an
    // execution role's session) — the very credential the script's own calls are then signed
    // with. The object store authorizes it against the policies in force on the role and
    // against the bucket's own policy, exactly as it authorizes any other signed request, so
    // a run whose role may not read its script never reaches the interpreter and reports the
    // script access it was refused rather than something the script went on to be refused.
    // `None` where no session could be minted — nothing is known about the role — in which
    // case the read is made with the credential the framework hands this emulator's handlers,
    // as every other in-process read between two emulated services is.
    let identity = execution_role_identity(
        engine,
        ctx.credential_store(),
        &plan.role_arn,
        &plan.job_name,
        SystemTime::now(),
    );
    let source = match read_script(ctx.callback(), region, &bucket, &key, identity.clone()).await {
        Ok(source) => source,
        Err(ScriptNotRead::Denied) => {
            ::tracing::debug!(
                job = %plan.job_name,
                run = %plan.run_id,
                location = ?location,
                "the object store refused a Glue job run's script to the role the job runs \
                 as, so nothing was run"
            );
            return Some(JobRunOutcome::Failed(job_run_script_not_accessible(
                location,
            )));
        }
        Err(ScriptNotRead::Unreadable(detail)) => {
            ::tracing::debug!(
                job = %plan.job_name,
                run = %plan.run_id,
                location = ?location,
                detail = %detail,
                "a Glue job run's script could not be read"
            );
            return Some(JobRunOutcome::Failed(job_run_script_unreadable(
                location, &detail,
            )));
        }
    };

    // How many scripts run at once is bounded, and the wait for a permit is bounded twice:
    // by its own span and by what the CALL has left — so a request is never held past
    // [`JOB_SCRIPT_CALL_CEILING`] waiting for an interpreter. A run that waited the whole
    // wait has not been performed.
    // The permit is OWNED and travels with the script's own thread, so it is given back when
    // that thread ends rather than when this caller stops waiting for it.
    let permit_wait = JOB_SCRIPT_PERMIT_WAIT.min(left_of_the_call);
    let permit = match ::tokio::time::timeout(
        permit_wait,
        ::std::sync::Arc::clone(&JOB_SCRIPT_RUN_PERMITS).acquire_owned(),
    )
    .await
    {
        Ok(Ok(permit)) => permit,
        _ => {
            ::tracing::debug!(
                job = %plan.job_name,
                run = %plan.run_id,
                "a Glue job run found no free interpreter for its script, so it stays owed one"
            );
            return None;
        }
    };
    // The calls the script itself makes are made as the role the job runs as, over the same
    // in-process callback, and the script never sees the credential. It is the SAME session
    // the script was fetched with: the mint holds one per role and run environment, so the
    // fetch and the run speak for one identity.
    let access = function_sdk_access(engine, ctx, region, &plan.role_arn, &plan.job_name);
    Some(run_script(plan, &source, &key, access, permit).await)
}

/// Why a job's script was not read: the two answers that are told apart, because the service
/// words them differently ([`job_run_script_not_accessible`] and
/// [`job_run_script_unreadable`]).
#[derive(Debug, Clone)]
enum ScriptNotRead {
    /// The object store answered that the read is NOT PERMITTED to the role the job runs as.
    Denied,
    /// Anything else the read established: the store holds no such object, the object is
    /// past the bound, it is not text, or the store answered something else about it.
    Unreadable(String),
}

/// The script's own source, read from the object the job's `ScriptLocation` names, as
/// `identity` — the session of the job's execution role, or the emulator's own credential
/// where there is none.
///
/// The read is RANGED, so at most [`MAX_JOB_SCRIPT_BYTES`] plus the one byte that shows the
/// object is past the bound is ever buffered, and a script that is not text is one no
/// interpreter can compile. What the store ANSWERED decides which refusal the run reports,
/// through the tree's one reading of an object store's answer
/// ([`::service_common::s3_object_read::OBJECT_DENIED_CODES`]).
async fn read_script(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    bucket: &str,
    key: &str,
    identity: Option<CallerIdentity>,
) -> Result<String, ScriptNotRead> {
    let client = match identity {
        Some(identity) => ::service_common::s3_client::s3_client_signed_as(
            callback,
            region,
            ::aws_sdk_s3::config::Credentials::new(
                identity.access_key_id,
                identity.secret_access_key,
                Some(identity.session_token),
                None,
                JOB_RUN_CREDENTIAL_SOURCE,
            ),
        ),
        None => ::service_common::s3_client::s3_client(callback, region),
    };
    let object = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .range(format!("bytes=0-{MAX_JOB_SCRIPT_BYTES}"))
        .send()
        .await
        .map_err(|error| {
            let code = error.code().unwrap_or_default().to_string();
            if ::service_common::s3_object_read::OBJECT_DENIED_CODES.contains(&code.as_str()) {
                return ScriptNotRead::Denied;
            }
            ScriptNotRead::Unreadable(format!(
                "the object store answered {:?}: {}",
                error.code(),
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            ))
        })?;
    let body = object
        .body
        .collect()
        .await
        .map_err(|error| {
            ScriptNotRead::Unreadable(format!("the script did not arrive in full: {error}"))
        })?
        .into_bytes();
    if body.len() > MAX_JOB_SCRIPT_BYTES {
        return Err(ScriptNotRead::Unreadable(format!(
            "the script is past the {MAX_JOB_SCRIPT_BYTES} bytes one job script may hold"
        )));
    }
    String::from_utf8(body.to_vec())
        .map_err(|_| ScriptNotRead::Unreadable("the script is not text".to_string()))
}

/// Run `source` as the job's script, on a thread of its own, and answer what it came to.
///
/// The interpreter is synchronous and CPU-bound, so it runs on a thread with a stack of its
/// own rather than on the async runtime, and the caller's wait for it is the host backstop
/// ([`JOB_SCRIPT_HOST_WAIT`]).
///
/// The span the engine bounds the run itself by is read against a clock the RUN drives
/// ([`JOB_SCRIPT_CLOCK_TICK`]) rather than against this host's: what the run comes to rest in
/// is then decided by what the script did — the events it delivered and the waits it asked
/// for — and not by how fast this host happened to be running it. That same clock states what
/// the run may take of THIS host ([`JOB_SCRIPT_HOST_CEILING`]), which is the bound the engine
/// stops it on.
///
/// `permit` is this run's share of [`MAX_CONCURRENT_JOB_SCRIPT_RUNS`] and is MOVED into that
/// thread, so it is given back when the thread ends — a run the caller has given up waiting
/// for still holds its share until it really stops, which is what keeps the count of
/// scripts running at once inside the bound.
async fn run_script(
    plan: &JobScriptPlan,
    source: &str,
    key: &str,
    access: Option<AwsAccess>,
    permit: ::tokio::sync::OwnedSemaphorePermit,
) -> JobRunOutcome {
    let budget = job_script_budget(plan.timeout_minutes);
    // Where the script is reported to run from: the path a job script is placed at on the
    // worker that runs it, which is what its own traceback frames name.
    let path = format!("/tmp/{}", key.split('/').next_back().unwrap_or(key));
    let event = ::serde_json::json!({
        "path": path,
        "source": source,
        "arguments": plan.command_line_arguments(),
    })
    .to_string();
    let modules = vec![
        PackageModule {
            path: format!("{SCRIPT_ENTRY_MODULE}.py"),
            source: SCRIPT_ENTRY_SOURCE.to_string(),
        },
        PackageModule {
            path: "awsglue/__init__.py".to_string(),
            source: AWSGLUE_SOURCE.to_string(),
        },
        PackageModule {
            path: "awsglue/utils.py".to_string(),
            source: AWSGLUE_UTILS_SOURCE.to_string(),
        },
    ];
    let run_id = plan.run_id.clone();
    let handler = format!("{SCRIPT_ENTRY_MODULE}.{SCRIPT_ENTRY_FUNCTION}");
    let (sender, receiver) = ::tokio::sync::oneshot::channel();
    let spawned = ::std::thread::Builder::new()
        .name("glue-job-script".to_string())
        .stack_size(JOB_SCRIPT_STACK_BYTES)
        .spawn(move || {
            // Held for as long as the script runs, whoever is still waiting for it.
            let _permit = permit;
            let finished = python::run_against(
                &python::Run {
                    modules: &modules,
                    handler: &handler,
                    // A Glue job script reads its parameters off its command line, so the run
                    // is given no environment of its own.
                    environment: &::std::collections::BTreeMap::new(),
                    event: Some(&event),
                    request_id: &run_id,
                    invoked_function_arn: "",
                    client_context: None,
                    // The tenant context an AWS Lambda INVOCATION carries, which that
                    // runtime reports to its handler as `context.tenant_id`: a Glue job run
                    // carries no such value — neither `StartJobRun` nor the modelled
                    // `JobRun` has a tenant member — so there is none to report, and nothing
                    // of this run's scope rests on it. What scopes the run is
                    // request-derived and passed beside it: the role session its own AWS
                    // calls are made as, built from the job's `Role`
                    // ([`JobScriptPlan::role_arn`]), and the Region of the endpoint the
                    // start arrived at, both of which reach the guest through `aws`.
                    tenant_id: None,
                    budget,
                    aws: access.as_ref(),
                },
                // The clock the run's own span is read against: one the RUN drives, a tick
                // per traced event it delivers and the waits it asks for, so the state the
                // run comes to rest in is what the script came to rather than what this
                // host's load made of it — stating what the run may take of THIS host
                // besides, which is the bound the engine stops it on whatever its own clock
                // reads.
                &python::Clock::driven(JOB_SCRIPT_CLOCK_TICK).within(JOB_SCRIPT_HOST_CEILING),
            );
            let _ = sender.send(finished);
        });
    if let Err(error) = spawned {
        return JobRunOutcome::EndedInError(format!(
            "the thread the job script runs on could not be started: {error}"
        ));
    }
    match ::tokio::time::timeout(JOB_SCRIPT_HOST_WAIT, receiver).await {
        Ok(Ok(finished)) => read_outcome(&finished.completion),
        Ok(Err(_)) => JobRunOutcome::EndedInError(
            "the thread the job script ran on ended without reporting".to_string(),
        ),
        // The thread has still not answered a grace period past the host time one run may
        // take — which the engine has by then stopped the run on — so it is given up on and
        // the run reports that this emulator has no outcome for it. Nothing of the script's
        // own span ran out, so it is not reported as terminated for taking too long.
        Err(_elapsed) => JobRunOutcome::EndedInError(job_run_script_unanswered()),
    }
}

/// What the run reports for a script the interpreter answered for.
fn read_outcome(completion: &python::Completion) -> JobRunOutcome {
    match completion {
        // A Glue job script is a program rather than a function, so what it ANSWERED is not
        // part of the run: a script that ran to its end exited cleanly.
        python::Completion::Returned(_) | python::Completion::Streamed { .. } => {
            JobRunOutcome::Succeeded
        }
        python::Completion::Raised {
            error_type,
            error_message,
            ..
        } => JobRunOutcome::Failed(script_failure(error_type, error_message)),
        python::Completion::Stopped(python::Stop::Deadline) => timed_out(),
        python::Completion::Stopped(python::Stop::Steps) => JobRunOutcome::Failed(
            job_run_script_bound_reached("the step budget this emulator gives a job script"),
        ),
        python::Completion::Stopped(python::Stop::Memory) => JobRunOutcome::Failed(
            job_run_script_bound_reached("the memory ceiling this emulator gives a job script"),
        ),
        python::Completion::Stopped(python::Stop::Response) => JobRunOutcome::Failed(
            job_run_script_bound_reached("the size of the value a job script may answer with"),
        ),
        // Not a bound of the script's own run: the host time one run may take
        // ([`JOB_SCRIPT_HOST_CEILING`]) is this emulator giving up on the execution, so the
        // run ends in the state a SERVICE error puts it in rather than in one that says the
        // script was terminated or failed.
        python::Completion::Stopped(python::Stop::HostCeiling) => {
            JobRunOutcome::EndedInError(job_run_script_host_ceiling_reached())
        }
        python::Completion::Internal(detail) => JobRunOutcome::EndedInError(detail.clone()),
    }
}

/// The reason a run whose script RAISED reports: the exception's own type and message, as
/// the interpreter reported them, which is the script's own failure and not this emulator's
/// words for it.
fn script_failure(error_type: &str, error_message: &str) -> String {
    if error_message.is_empty() {
        return error_type.to_string();
    }
    format!("{error_type}: {error_message}")
}

/// What a run whose script ran past the span its own run may last reports: the modelled
/// `TIMEOUT` state, carrying the ceiling that stopped it
/// ([`job_run_script_ceiling_reached`]).
///
/// The span is the RUN's own ([`JOB_SCRIPT_RUN_CEILING`]), so this is reported for a script
/// that really ran past it — one that WAITED past it — and not for one this host was slow to
/// get through.
fn timed_out() -> JobRunOutcome {
    JobRunOutcome::TimedOut(job_run_script_ceiling_reached())
}

/// The module this emulator runs the job's own script FROM, and the function in it the
/// engine calls.
const SCRIPT_ENTRY_MODULE: &str = "__glue_job_script";
/// See [`SCRIPT_ENTRY_MODULE`].
const SCRIPT_ENTRY_FUNCTION: &str = "run";

/// The Python source of that module: it runs the job's script the way a Python program is
/// run — as the `__main__` module, with the job's arguments on `sys.argv` — and lets
/// whatever the script raises travel out to the engine, which is what reports it.
///
/// Nothing here is a bound or a containment: both are the engine's, in host code no script
/// can reach.
const SCRIPT_ENTRY_SOURCE: &str = r#"
"""Runs a Glue job's own script, as a Python program with the job's arguments."""

import sys


def run(event, context):
    # The command line a job script resolves its arguments off: the script's own path
    # followed by the job's arguments, which is what AWS Glue hands the script.
    sys.argv = [event["path"]] + list(event["arguments"])
    # A script is run as a program, so its globals are those of the main module and its
    # `if __name__ == "__main__":` guard holds, exactly as on a Glue worker.
    script_globals = {"__name__": "__main__", "__file__": event["path"]}
    exec(compile(event["source"], event["path"], "exec"), script_globals)
    return None
"#;

/// `awsglue`: the package AWS Glue ships on a job script's import path. Only the part this
/// emulator can serve is here ([`AWSGLUE_UTILS_SOURCE`]); importing anything else of it
/// raises, as it does in an environment that does not have it.
const AWSGLUE_SOURCE: &str = r#"
"""The `awsglue` package, as this emulator supplies it on a job script's import path."""
"#;

/// `awsglue.utils`: the argument resolution a Glue job script reads its parameters through.
///
/// "`getResolvedOptions(args, options)`" takes `sys.argv` and the names the script wants,
/// and answers a dictionary of them — AWS documents it as the way "to retrieve the arguments
/// that were passed", and a job argument is named on the command line as `--name value`. A
/// name the line does not carry raises `GlueArgumentError`, because the script asked for a
/// parameter the run was not given.
const AWSGLUE_UTILS_SOURCE: &str = r#"
"""Resolving a Glue job's own arguments off the command line it was run with."""


class GlueArgumentError(Exception):
    """A requested job argument the run was not given."""


def _named_arguments(args):
    """Every `--name value` (or `--name=value`) pair of a command line, by name."""
    named = {}
    index = 0
    while index < len(args):
        token = args[index]
        index += 1
        if not token.startswith("--"):
            continue
        name = token[2:]
        if "=" in name:
            name, value = name.split("=", 1)
            named[name] = value
            continue
        if index < len(args) and not args[index].startswith("--"):
            named[name] = args[index]
            index += 1
        else:
            named[name] = ""
    return named


def getResolvedOptions(args, options):
    """The job arguments `options` names, read off the command line `args`."""
    named = _named_arguments(list(args))
    resolved = {}
    for option in options or []:
        name = option[2:] if option.startswith("--") else option
        if name not in named:
            raise GlueArgumentError("argument --%s is required" % name)
        resolved[name] = named[name]
    return resolved
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// What the run REPORTS for each thing the engine can answer with: the script's own
    /// outcome for everything the script itself did, and the state a SERVICE error puts a run
    /// in for the bound that is this emulator giving up on the host time one run may take —
    /// which is never reported as a run terminated for taking too long.
    ///
    /// One row per completion the engine has, so a bound added to the engine has to be
    /// answered here too.
    #[test]
    fn a_host_give_up_is_not_reported_as_a_run_terminated_for_taking_too_long() {
        // (case, what the engine answered, the state the run comes to rest in, its reason)
        let cases: Vec<(&str, python::Completion, &str, Option<String>)> = vec![
            (
                "a script that ran to its end",
                python::Completion::Returned("null".to_string()),
                JOB_RUN_STATE_SUCCEEDED,
                None,
            ),
            (
                "a script that raised",
                python::Completion::Raised {
                    error_type: "ValueError".to_string(),
                    error_message: "the input partition is malformed".to_string(),
                    stack_trace: Vec::new(),
                },
                JOB_RUN_STATE_FAILED,
                Some("ValueError: the input partition is malformed".to_string()),
            ),
            (
                "a script that ran past the span its own run may last",
                python::Completion::Stopped(python::Stop::Deadline),
                JOB_RUN_STATE_TIMEOUT,
                Some(job_run_script_ceiling_reached()),
            ),
            (
                "a script that spent its step budget",
                python::Completion::Stopped(python::Stop::Steps),
                JOB_RUN_STATE_FAILED,
                Some(job_run_script_bound_reached(
                    "the step budget this emulator gives a job script",
                )),
            ),
            (
                "a script that spent its memory ceiling",
                python::Completion::Stopped(python::Stop::Memory),
                JOB_RUN_STATE_FAILED,
                Some(job_run_script_bound_reached(
                    "the memory ceiling this emulator gives a job script",
                )),
            ),
            (
                "a script that answered with more than one may",
                python::Completion::Stopped(python::Stop::Response),
                JOB_RUN_STATE_FAILED,
                Some(job_run_script_bound_reached(
                    "the size of the value a job script may answer with",
                )),
            ),
            (
                "a run this emulator gave up on at the host time one run may take",
                python::Completion::Stopped(python::Stop::HostCeiling),
                JOB_RUN_STATE_ERROR,
                Some(job_run_script_host_ceiling_reached()),
            ),
            (
                "an interpreter that could not be driven at all",
                python::Completion::Internal("the interpreter could not be built".to_string()),
                JOB_RUN_STATE_ERROR,
                Some("the interpreter could not be built".to_string()),
            ),
        ];
        for (case, completion, state, reason) in cases {
            let outcome = read_outcome(&completion);
            assert_eq!(
                outcome.job_run_state(),
                state,
                "case {case:?}: expected the run to come to rest in {state}, got {outcome:?}"
            );
            assert_eq!(
                outcome.error_message(),
                reason,
                "case {case:?}: expected the reason {reason:?}, got {outcome:?}"
            );
        }
    }

    /// And the run of a thread the call gave up waiting for reports the same service error
    /// rather than a terminated script: nothing of the script's own span ran out, so its
    /// reason says this emulator has no outcome for the run.
    #[test]
    fn a_thread_that_never_answered_is_reported_as_a_run_with_no_outcome() {
        let reason = job_run_script_unanswered();
        let outcome = JobRunOutcome::EndedInError(reason.clone());
        assert_eq!(
            outcome.job_run_state(),
            JOB_RUN_STATE_ERROR,
            "a run nothing answered for ends in {JOB_RUN_STATE_ERROR}, got {outcome:?}"
        );
        assert_eq!(
            outcome.error_message(),
            Some(reason),
            "the run carries the reason this emulator has no outcome for it, got {outcome:?}"
        );
        assert_ne!(
            outcome.job_run_state(),
            JOB_RUN_STATE_TIMEOUT,
            "a thread the call gave up on is not a script terminated for taking too long"
        );
    }
}
