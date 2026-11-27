// SPDX-License-Identifier: CC0-1.0

//! RUNNING the code of one statement on an interactive session's Spark runtime: what it runs
//! in, and what the statement it belongs to comes to rest in.
//!
//! A session is "the period in which a remote Spark runtime environment is running"
//! (`Session`) and a statement is "the statement or request for a particular action to occur
//! in a session" (`Statement`), submitted as `RunStatement$Code`, "the statement code to be
//! run". So a statement IS one execution of that code on the session's runtime, and the state
//! it comes to rest in is whatever that execution came to: the modelled `AVAILABLE` for code
//! that ran to its end, whose output is then readable, and the modelled `ERROR` carrying the
//! CODE's own exception for one that raised.
//!
//! That is what this module does, and nothing about it is synthesised:
//!
//! * the code is the caller's own, exactly as `RunStatement` recorded it
//!   ([`StatementEntry::code`]);
//! * it is EXECUTED by the contained Python interpreter this workspace already embeds (the
//!   Lambda service crate's `state::execution::python`), with the AWS Glue ETL library and the
//!   Spark runtime under it on the import path ([`crate::state::glue_etl`]) — which is what a
//!   session's cell finds in its own environment, where `sc`, `spark`, `glueContext` and `job`
//!   are already bound — so a cell's `boto3` call reaches this same emulator and its effects
//!   are really made. No second interpreter is stood up here, and no part of a cell is
//!   interpreted by this crate;
//! * what the statement REPORTS is what that execution answered: the terminal `StatementState`
//!   is decided by the outcome ([`StatementOutcome`]), `StatementOutput$Data` is the text the
//!   code itself wrote, and `ErrorName`, `ErrorValue` and `Traceback` of a failed one are the
//!   exception the code raised.
//!
//! # Where the code runs from, and when
//!
//! The execution is HANDED OFF where the statement is SUBMITTED — by `RunStatement`, or by a
//! later call that finds it still owed — to a task of this process, with no store guard held
//! and with no request waiting on it. That is what `RunStatement` is: it "runs the statement"
//! and answers the `Id` it minted, while the code itself goes on running, so the call that
//! submits a cell is not held for as long as the cell takes and a client polling
//! `GetStatement` observes the statement WHILE its code is running rather than only before and
//! after it. A statement whose code has not been performed yet is marked
//! [`StatementEntry::awaiting_execution`], so an operation that reads a session's statements
//! hands off what was already owed rather than reporting a statement nothing is running: the
//! execution was owed the instant the statement was recorded, whoever recorded it.
//!
//! A session takes every statement submitted to it while its runtime is up, and runs them ONE
//! AT A TIME: a statement is claimed only while no statement of that session is carrying its
//! runtime ([`SessionEntry::claim_statement_awaiting_execution`]), the oldest one awaiting an
//! execution first, so a cell submitted while another is in flight is QUEUED — recorded, owed
//! an execution, and taken up by the next call that finds it owed once the one ahead of it has
//! answered.
//!
//! WHAT the statement then REPORTS is the clock's, not this module's: the state the execution
//! came to is owed one span after the statement has been [`STATEMENT_STATE_RUNNING`], and
//! never before one span after the execution itself answered
//! ([`StatementEntry::record_execution`]) — so the statement passes through `RUNNING` for as
//! long as its code takes, and a client polling it observes every state it goes through.
//!
//! # Containment
//!
//! The code never reaches this host. It is compiled and run ONLY inside the contained
//! interpreter this workspace embeds — the same engine a Glue job script and an AWS Lambda
//! `python3.x` handler run in (the Lambda crate's `state::execution::python`) — whose
//! containment is the ENGINE's, in host code no cell can reach: the interpreter is built with
//! the host-reaching features compiled out, its native modules are an ALLOW-LIST of what the
//! frozen standard library needs (so no host socket, subprocess, memory map, host library
//! loader or host thread is registered at all, however a cell rebinds or deletes names), and
//! the modules a program expects of its operating system are that engine's own, whose host
//! calls refuse. Nothing of this crate's own is handed to a cell: no host path, no host
//! environment variable and no credential. The ONE way out of the interpreter is the AWS SDK
//! seam the engine supplies ([`function_sdk_access`]), whose requests this emulator signs
//! itself as a session of the SESSION's `Role` and dispatches over its OWN in-process callback
//! — so a cell's `boto3` call reaches this emulator and nothing else, and the cell never sees
//! the credential.
//!
//! This module adds no host surface of its own, and the statements below state only what the
//! code here enforces.
//!
//! # Bounds
//!
//! A cell is caller-supplied code, so every bound is this emulator's and none of them is
//! reachable from the cell: how many cells run at once and how many handed-off executions can
//! be in flight at a time ([`MAX_CONCURRENT_STATEMENT_RUNS`], whose permit is taken before
//! anything is spawned, with no wait), how many executions one call hands off
//! ([`MAX_STATEMENT_RUNS_PER_CALL`], and only while the call has not already spent
//! [`STATEMENT_CALL_CEILING`]), how long one cell's own run may last
//! ([`STATEMENT_RUN_CEILING`]), how much of THIS HOST's time one run may take
//! ([`STATEMENT_HOST_CEILING`], which the engine stops the run on) and how long the task waits
//! for the thread it runs on ([`STATEMENT_HOST_WAIT`]), beside the step, memory, recursion, log
//! and answer ceilings the interpreter is given ([`statement_budget`]) — the same ceilings this
//! emulator gives any run of caller-supplied Python here ([`super::job_script`]).
//!
//! The deadline and the step budget are the ENGINE's latched ones, read against the clock this
//! module hands the run, and the engine's own watch over that run reads the host ceiling from
//! this host's clock — so a cell that delivers no traced event, or that catches what the stop
//! raised, is stopped all the same. A cell's blocking waits are the engine's clamped ones, and
//! the thread one run holds is given up on at [`STATEMENT_HOST_WAIT`] while its permit stays
//! taken until that thread really ends, so neither a cell that waits nor one this emulator gave
//! up on takes the count of cells running at once past its bound.

use ::std::time::{Duration, Instant, SystemTime};

use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::aws_api::CallerIdentity;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::execution_role_session::execution_role_identity;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::python;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    function_sdk_access, AwsAccess,
};

use super::*;

/// The longest ONE run of a statement's code may last in the RUN'S OWN time.
///
/// Neither `RunStatement` nor the modelled `Session` carries a timeout for one statement — the
/// documented timeouts are the SESSION's, in minutes (`CreateSessionRequest$Timeout`,
/// `$IdleTimeout`) — so this is the span this emulator gives one run of a cell, and it is the
/// same span it gives one run of a job's script ([`JOB_SCRIPT_RUN_CEILING`]), for the reason
/// stated there: it is a span of the RUN's own time, spent by the waits the code asks for and
/// by one tick per traced event it delivers, so what a statement comes to rest in is the same
/// on an idle host and on a loaded one.
pub const STATEMENT_RUN_CEILING: Duration = JOB_SCRIPT_RUN_CEILING;

/// How far ONE reading of a statement run's clock moves it: the tick every run of
/// caller-supplied Python here is read against ([`JOB_SCRIPT_CLOCK_TICK`]).
pub const STATEMENT_CLOCK_TICK: Duration = JOB_SCRIPT_CLOCK_TICK;

/// How much of THIS HOST's own time ONE run of a cell may take, past which the engine stops it
/// whatever its own clock reads: the ceiling every run of caller-supplied Python here is
/// stopped on ([`JOB_SCRIPT_HOST_CEILING`]), which covers the whole span the engine may take in
/// the run's own time.
pub const STATEMENT_HOST_CEILING: Duration = JOB_SCRIPT_HOST_CEILING;

/// How long the task that performs ONE execution waits for the thread that execution runs on:
/// the host ceiling plus the grace the engine's own stop needs to land
/// ([`JOB_SCRIPT_HOST_WAIT`]).
pub const STATEMENT_HOST_WAIT: Duration = JOB_SCRIPT_HOST_WAIT;

/// How many statement cells run at once in this process, which is also how many handed-off
/// executions can be in flight at a time.
///
/// CHOSEN: a session actions one statement at a time, so this is a bound on how many SESSIONS
/// are running a cell together — enough for the sessions a client drives beside each other
/// while a cell that waits holds one of them — and a statement that finds no free interpreter
/// is handed off by NOBODY: the permit is taken before anything is spawned
/// ([`hand_off_statement`]), so the statement stays [`StatementEntry::awaiting_execution`] and
/// a later call hands it off. Nothing waits for a permit.
pub const MAX_CONCURRENT_STATEMENT_RUNS: usize = 8;

/// The permits [`MAX_CONCURRENT_STATEMENT_RUNS`] hands out, for the whole process.
///
/// Taken with no wait at all, BEFORE an execution is handed off, and held as an owned permit
/// MOVED INTO the thread that runs the cell — so it is given back when that thread really
/// ends, including for a statement nobody is reading any more.
static STATEMENT_RUN_PERMITS: ::std::sync::LazyLock<::std::sync::Arc<::tokio::sync::Semaphore>> =
    ::std::sync::LazyLock::new(|| {
        ::std::sync::Arc::new(::tokio::sync::Semaphore::new(MAX_CONCURRENT_STATEMENT_RUNS))
    });

/// How many statement executions ONE call hands off.
///
/// CHOSEN: a session actions one statement at a time, so a call normally finds one owed; this
/// is the bound on what a call does when it finds more, leaving the rest to the next call.
pub const MAX_STATEMENT_RUNS_PER_CALL: usize = 4;

/// How long ONE call spends handing off owed executions before it leaves the rest to the next
/// call: the ceiling on what a REQUEST costs, beside the ceiling on what one cell's own run
/// costs ([`STATEMENT_RUN_CEILING`]). No execution in flight is ever waited for.
pub const STATEMENT_CALL_CEILING: Duration = Duration::from_secs(5);

/// The stack a cell's thread is given: the one every run of caller-supplied Python here gets,
/// well above the interpreter's clamped recursion limit.
const STATEMENT_STACK_BYTES: usize = JOB_SCRIPT_STACK_BYTES;

/// What one statement run may spend: the span above, beside the step, memory, recursion, log
/// and answer ceilings every run of caller-supplied Python here is given
/// ([`super::job_script`], whose ceilings these are).
#[must_use]
pub fn statement_budget() -> python::Budget {
    python::Budget {
        wall_clock: STATEMENT_RUN_CEILING,
        steps: MAX_JOB_SCRIPT_STEPS,
        memory_bytes: MAX_JOB_SCRIPT_MEMORY_BYTES,
        ephemeral_bytes: MAX_JOB_SCRIPT_SCRATCH_BYTES,
        log_bytes: MAX_STATEMENT_OUTPUT_BYTES,
        response_bytes: MAX_JOB_SCRIPT_ANSWER_BYTES,
        recursion_limit: MAX_JOB_SCRIPT_RECURSION,
    }
}

/// How much of a cell's own output is kept, which is what `StatementOutput$Data` reports.
///
/// CHOSEN: `StatementOutputData$TextPlain` is "the code execution output in text format" and
/// the model bounds it nowhere, so this is the bound on what one statement holds — and on what
/// a read of it answers with. The interpreter enforces it, so nothing larger is ever buffered.
pub const MAX_STATEMENT_OUTPUT_BYTES: usize = 64 * 1024;

/// The ROLE-SESSION NAME a statement's session of the session's `Role` is opened under.
///
/// CHOSEN, not recorded: `CreateSession$Role` is "the IAM Role ARN" a session runs as, and the
/// calls a cell makes are made as a session of it, which has to be opened under some name.
/// It names the service and the kind of session and carries no value of one session or one
/// statement, the way the name a job runner's session is opened under does
/// ([`JOB_RUN_ROLE_SESSION_NAME`], which IS recorded).
pub const SESSION_ROLE_SESSION_NAME: &str = "GlueSessionRunnerSession";

/// What one statement's execution came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementOutcome {
    /// The code ran to its end. The text is everything it wrote, as the interpreter captured
    /// it, which is what `StatementOutput$Data` reports.
    Available(String),
    /// It raised, or a bound stopped it: the error's name, its value, and the stack the
    /// interpreter reported — the three members `StatementOutput` carries for a failure.
    Errored {
        /// `StatementOutput$ErrorName`: "the name of the error in the output".
        name: String,
        /// `StatementOutput$ErrorValue`: "the error value of the output".
        value: String,
        /// `StatementOutput$Traceback`: "the traceback of the output".
        traceback: Vec<String>,
    },
}

impl StatementOutcome {
    /// The `StatementState` a statement whose execution came to this rests in: the modelled
    /// `AVAILABLE` for code that ran to its end — "the state while request is actioned", and
    /// the state whose output is readable — and the modelled `ERROR` for one that did not.
    #[must_use]
    pub fn statement_state(&self) -> &'static str {
        match self {
            Self::Available(_) => STATEMENT_STATE_AVAILABLE,
            Self::Errored { .. } => STATEMENT_STATE_ERROR,
        }
    }
}

/// Everything one execution of a statement's code needs, copied out of the store under its
/// guard so the execution itself holds none.
#[derive(Debug, Clone)]
pub struct StatementPlan {
    /// The session the statement was run on.
    pub session_id: String,
    /// The statement whose code this is.
    pub statement_id: i32,
    /// `RunStatement$Code`: "the statement code to be run", exactly as it was submitted.
    pub code: String,
    /// `Session$GlueVersion`, as the session carries it: it "determines the versions of Apache
    /// Spark and Python that Glue supports", so it is what decides the Spark the cell runs
    /// against ([`crate::state::glue_etl`]). A session that named none runs against the
    /// runtime's own.
    pub glue_version: String,
    /// The ARN of the role the cell's own calls are made as: `Session$Role`, through the
    /// crate's one reading of that member ([`glue_role_reference`]).
    pub role_arn: String,
}

impl StatementPlan {
    /// The plan for the statement `statement_id` of `session`, or `None` when the session
    /// holds no such statement.
    #[must_use]
    pub fn of(session: &SessionEntry, statement_id: i32) -> Option<Self> {
        let statement = session.statement(statement_id)?;
        Some(Self {
            session_id: session.id.clone(),
            statement_id,
            code: statement.code.clone(),
            glue_version: session.glue_version.clone().unwrap_or_default(),
            role_arn: glue_role_reference(&session.role).arn,
        })
    }
}

/// Hand off the code of every statement of `session_id` that is still awaiting an execution,
/// so each of those statements runs and records what its execution came to.
///
/// This is the ONE place a statement's code is run from, so a statement submitted by
/// `RunStatement` and one a later read finds still owed are executed the same way. Nothing of
/// it happens under the store's guard: the plan is copied out under the guard, the code runs
/// without it, and the outcome is recorded under it again.
///
/// The execution is the STATEMENT's work and not the request's, so this call does not wait for
/// it: each claimed execution is handed to a task of this process ([`hand_off_statement`]) and
/// the call answers. That is what makes a statement observable while it is in flight — it is
/// `WAITING`, then [`STATEMENT_STATE_RUNNING`] for as long as its code runs, and only one span
/// after that execution answered the state it came to
/// ([`StatementEntry::record_execution`]).
pub async fn perform_owed_session_statements<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    session_id: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let began = Instant::now();
    let mut handed_off = 0usize;
    while handed_off < MAX_STATEMENT_RUNS_PER_CALL && began.elapsed() < STATEMENT_CALL_CEILING {
        // What the next owed execution is, resolved under the guard; the guard is released
        // before anything is run.
        let plan = {
            let mut store = state.lock();
            let Some(session) = store.session_mut(region, session_id) else {
                break;
            };
            // Everything the clock already owed this session and its statements is
            // materialised first, so a statement that has already come to rest is not run.
            let now = SystemTime::now();
            session.settle_if_due(now);
            session.settle_due_statements(now);
            // The execution is CLAIMED under the guard, so two calls that find the same
            // statement owed one cannot both run it — and only one statement of a session
            // is ever claimed at a time, because a session runs the statements it has taken
            // one at a time, so everything submitted in the meantime stays queued.
            let Some(statement_id) = session.claim_statement_awaiting_execution(now) else {
                break;
            };
            match StatementPlan::of(session, statement_id) {
                Some(plan) => plan,
                // Nothing can be run without a plan, so the claim goes straight back.
                None => {
                    session.release_statement_claim(statement_id);
                    break;
                }
            }
        };

        // The credential the cell's own calls are made with is minted HERE, where the
        // request's own context is: it is the session of the session's `Role`
        // ([`execution_role_identity`], the one mint this workspace has for one), opened under
        // the name this service opens a session runner's session under
        // ([`SESSION_ROLE_SESSION_NAME`]) — so the principal that session speaks for is the one
        // a service names when it refuses one of the cell's calls — and the execution is given
        // it rather than reaching for a credential of its own.
        let identity = execution_role_identity(
            engine,
            ctx.credential_store(),
            &plan.role_arn,
            SESSION_ROLE_SESSION_NAME,
            SystemTime::now(),
        );
        let access = function_sdk_access(
            engine,
            ctx,
            region,
            &plan.role_arn,
            SESSION_ROLE_SESSION_NAME,
        )
        // The AWS SDK the cell's own calls are made with is the one the session's RUNTIME
        // supplies, which is the generation its `GlueVersion` pins
        // ([`super::bundled_sdk_of`]) — so a call of it travels the wire that SDK's client of
        // the called service speaks.
        .map(|access| access.bundling(super::bundled_sdk_of(&plan.glue_version)));
        if !hand_off_statement(&state, ctx.callback(), region, plan, identity, access) {
            break;
        }
        handed_off += 1;
    }
}

/// Hand ONE claimed execution to a task of this process, and answer whether it was taken on.
///
/// The cell is the STATEMENT's work: it is performed past the life of the request that claimed
/// it, so the caller is not held for as long as the code takes and the statement is observable
/// while it runs. Everything the execution needs is OWNED — the plan copied out of the store,
/// the Region, the store handle, the callback this emulator's services reach each other
/// through, and the credential the cell's own calls are signed with — so the task holds nothing
/// of the request.
///
/// What bounds the hand-off is the INTERPRETER PERMIT, taken HERE and before anything is
/// spawned ([`MAX_CONCURRENT_STATEMENT_RUNS`]). The permit is OWNED and travels with the
/// execution into the thread its code runs on, so it is given back when that thread ends.
///
/// `false` where the execution was NOT handed off — no async runtime to run it on, or no free
/// interpreter — in which case the claim goes straight back, so the statement stays owed its
/// execution and a later call hands it off rather than this one reporting an outcome no
/// execution produced.
fn hand_off_statement(
    state: &::engine::ServiceState<ComAmazonawsGlueAwsglueState>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    plan: StatementPlan,
    identity: Option<CallerIdentity>,
    access: Option<AwsAccess>,
) -> bool {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        release_statement_claim(state, region, &plan);
        return false;
    };
    let Ok(permit) = ::std::sync::Arc::clone(&STATEMENT_RUN_PERMITS).try_acquire_owned() else {
        ::tracing::debug!(
            session = %plan.session_id,
            statement = plan.statement_id,
            "a Glue statement found no free interpreter, so it stays owed an execution"
        );
        release_statement_claim(state, region, &plan);
        return false;
    };
    let performed = ::engine::ServiceState::clone(state);
    let callback = callback.clone();
    let region = region.to_string();
    let handle = runtime.spawn(async move {
        perform_handed_off_statement(&performed, &callback, &region, &plan, identity, access, permit)
            .await;
    });
    state.lock().record_handed_off_execution(handle);
    true
}

/// Perform ONE handed-off execution and record what it came to on the statement it belongs to.
///
/// The outcome is recorded under the store's guard, taken only once the execution has answered:
/// the state the statement comes to REST in is what that execution came to, due one span after
/// it has been [`STATEMENT_STATE_RUNNING`] and never before one span after the execution
/// answered ([`StatementEntry::record_execution`]). An execution nothing ran gives the claim
/// back instead, so a later call performs it.
async fn perform_handed_off_statement(
    state: &::engine::ServiceState<ComAmazonawsGlueAwsglueState>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    plan: &StatementPlan,
    identity: Option<CallerIdentity>,
    access: Option<AwsAccess>,
    permit: ::tokio::sync::OwnedSemaphorePermit,
) {
    let Some(outcome) = perform_statement(callback, region, plan, identity, access, permit).await
    else {
        // Nothing ran: the claim goes back, so a later call performs it rather than anything
        // reporting an outcome no execution produced.
        release_statement_claim(state, region, plan);
        return;
    };
    {
        let mut store = state.lock();
        let Some(session) = store.session_mut(region, &plan.session_id) else {
            return;
        };
        if let Some(statement) = session.statement_mut(plan.statement_id) {
            statement.record_execution(outcome.clone(), SystemTime::now());
        }
    }
    ::tracing::info!(
        session = %plan.session_id,
        statement = plan.statement_id,
        state = %outcome.statement_state(),
        "ran the code of a Glue interactive session's statement"
    );
}

/// Give the claim on `plan`'s execution back, for an execution nothing performed.
fn release_statement_claim(
    state: &::engine::ServiceState<ComAmazonawsGlueAwsglueState>,
    region: &str,
    plan: &StatementPlan,
) {
    let mut store = state.lock();
    if let Some(session) = store.session_mut(region, &plan.session_id) {
        session.release_statement_claim(plan.statement_id);
    }
}

/// Run ONE statement's code and answer what it came to, or `None` when nothing ran it.
///
/// Every answer is grounded in what really happened: the exception the code raised, or the
/// bound the interpreter stopped it at.
///
/// `identity` is the session of the session's `Role` the cell is run as, `access` the seam its
/// own AWS calls travel this emulator over — both built by the call that claimed this execution
/// ([`perform_owed_session_statements`]) — and `permit` this execution's share of
/// [`MAX_CONCURRENT_STATEMENT_RUNS`], which travels into the thread the code runs on.
async fn perform_statement(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    plan: &StatementPlan,
    identity: Option<CallerIdentity>,
    access: Option<AwsAccess>,
    permit: ::tokio::sync::OwnedSemaphorePermit,
) -> Option<StatementOutcome> {
    // A session's runtime is stood up BY Glue acting as the session's `Role`, so the cell is
    // admitted only once this emulator's own IAM has been asked whether Glue can assume that
    // role at all: a role whose trust policy does not admit the Glue service principal backs
    // no runtime, and the statement fails before any of its code is executed. Only what IAM
    // ANSWERED fails it, so a role nothing can be shown about — including an emulator with no
    // IAM to ask — is admitted. Awaited with no store guard held.
    if glue_role_is_unassumable_through(callback, region, &plan.role_arn).await {
        ::tracing::debug!(
            session = %plan.session_id,
            statement = plan.statement_id,
            "a Glue session's role cannot be assumed by Glue, so no statement of it was run"
        );
        return Some(StatementOutcome::Errored {
            name: STATEMENT_RUNTIME_ERROR_NAME.to_string(),
            value: messages::session_role_not_assumable(&plan.role_arn),
            traceback: Vec::new(),
        });
    }
    // The identity the cell is run as is the one its own calls are signed with; the cell never
    // sees the credential.
    let _ = identity;
    Some(run_cell(plan, access, permit).await)
}

/// Run `plan`'s code as the session's cell, on a thread of its own, and answer what it came to.
///
/// The interpreter is synchronous and CPU-bound, so it runs on a thread with a stack of its own
/// rather than on the async runtime, and the wait for it is the host backstop
/// ([`STATEMENT_HOST_WAIT`]).
///
/// The span the engine bounds the run itself by is read against a clock the RUN drives
/// ([`STATEMENT_CLOCK_TICK`]) rather than against this host's: what the statement comes to rest
/// in is then decided by what the code did — the events it delivered and the waits it asked for
/// — and not by how fast this host happened to be running it. The waits it asks for really PASS
/// ([`python::Clock::waiting`]): a statement is observed by a client while it is in flight, so
/// code that waits keeps its statement `RUNNING` for that span. That same clock states what the
/// run may take of THIS host ([`STATEMENT_HOST_CEILING`]), which is the bound the engine stops
/// it on.
async fn run_cell(
    plan: &StatementPlan,
    access: Option<AwsAccess>,
    permit: ::tokio::sync::OwnedSemaphorePermit,
) -> StatementOutcome {
    let budget = statement_budget();
    let event = ::serde_json::json!({ "code": plan.code }).to_string();
    let modules = ::std::iter::once(PackageModule {
        path: format!("{CELL_ENTRY_MODULE}.py"),
        source: CELL_ENTRY_SOURCE.to_string(),
    })
    // The AWS Glue ETL library and the Spark runtime under it, which is what a cell resolves
    // its own imports against ([`crate::state::glue_etl`]).
    .chain(crate::state::glue_etl_modules(&plan.glue_version))
    .collect::<Vec<_>>();
    let request_id = format!("{}-{}", plan.session_id, plan.statement_id);
    let handler = format!("{CELL_ENTRY_MODULE}.{CELL_ENTRY_FUNCTION}");
    let (sender, receiver) = ::tokio::sync::oneshot::channel();
    let spawned = ::std::thread::Builder::new()
        .name("glue-session-statement".to_string())
        .stack_size(STATEMENT_STACK_BYTES)
        .spawn(move || {
            // Held for as long as the cell runs, whoever is still waiting for it.
            let _permit = permit;
            let finished = python::run_against(
                &python::Run {
                    modules: &modules,
                    handler: &handler,
                    // A cell reads its own code off the event it is given, so the run is given
                    // no environment of its own.
                    environment: &::std::collections::BTreeMap::new(),
                    architecture:
                        ::service_com_amazonaws_lambda_awsgirapiservice::state::DEFAULT_ARCHITECTURE,
                    event: Some(&event),
                    request_id: &request_id,
                    invoked_function_arn: "",
                    client_context: None,
                    // A statement carries no tenant member in the model, so there is none to
                    // report. What scopes the run is passed beside it: the role session its own
                    // AWS calls are made as, built from the session's `Role`.
                    tenant_id: None,
                    // Nor an Amazon Cognito identity: a cell is this service's own run, made
                    // by no identity a pool vended credentials to.
                    cognito_identity: None,
                    budget,
                    aws: access.as_ref(),
                    // A cell is no Lambda durable execution's run.
                    durable: None,
                    // And it mounts no Amazon EFS file system: a cell runs in the session this
                    // service holds, not in a function configured with one.
                    file_systems: &[],
                },
                &python::Clock::driven(STATEMENT_CLOCK_TICK)
                    .waiting()
                    .within(STATEMENT_HOST_CEILING),
            );
            let _ = sender.send(finished);
        });
    if let Err(error) = spawned {
        return StatementOutcome::Errored {
            name: STATEMENT_RUNTIME_ERROR_NAME.to_string(),
            value: format!("the thread the statement runs on could not be started: {error}"),
            traceback: Vec::new(),
        };
    }
    match ::tokio::time::timeout(STATEMENT_HOST_WAIT, receiver).await {
        Ok(Ok(finished)) => read_outcome(&finished),
        Ok(Err(_)) => StatementOutcome::Errored {
            name: STATEMENT_RUNTIME_ERROR_NAME.to_string(),
            value: "the thread the statement ran on ended without reporting".to_string(),
            traceback: Vec::new(),
        },
        // The thread has still not answered a grace period past the host time one run may take
        // — which the engine has by then stopped the run on — so it is given up on and the
        // statement reports that this emulator has no outcome for it.
        Err(_elapsed) => StatementOutcome::Errored {
            name: STATEMENT_RUNTIME_ERROR_NAME.to_string(),
            value: STATEMENT_UNANSWERED.to_string(),
            traceback: Vec::new(),
        },
    }
}

/// What the statement reports for a cell the interpreter answered for.
fn read_outcome(finished: &python::Finished) -> StatementOutcome {
    match &finished.completion {
        // A cell is a program rather than a function, so what the module ANSWERED is not part
        // of the statement: what it reports is the text the code itself wrote.
        python::Completion::Returned(_) | python::Completion::Streamed { .. } => {
            StatementOutcome::Available(finished.log.clone())
        }
        python::Completion::Raised {
            error_type,
            error_message,
            stack_trace,
            ..
        } => StatementOutcome::Errored {
            // The exception's own type and message, as the interpreter reported them, which is
            // the cell's own failure and not this emulator's words for it.
            name: error_type.clone(),
            value: error_message.clone(),
            traceback: stack_trace.clone(),
        },
        python::Completion::Stopped(stop) => StatementOutcome::Errored {
            name: STATEMENT_RUNTIME_ERROR_NAME.to_string(),
            value: statement_bound_reached(*stop),
            traceback: Vec::new(),
        },
        python::Completion::Internal(detail) => StatementOutcome::Errored {
            name: STATEMENT_RUNTIME_ERROR_NAME.to_string(),
            value: detail.clone(),
            traceback: Vec::new(),
        },
    }
}

/// `StatementOutput$ErrorName` for a failure that is not the cell's own exception: a bound this
/// emulator stopped the run at, or a run it has no outcome for. The kind of the error is that
/// the runtime ended the code rather than the code failing.
pub const STATEMENT_RUNTIME_ERROR_NAME: &str = "RuntimeError";

/// `StatementOutput$ErrorValue` of a statement whose cell a BOUND of this emulator stopped.
///
/// It names the bound that stopped the run rather than claiming an outcome the code produced.
#[must_use]
pub fn statement_bound_reached(stop: python::Stop) -> String {
    match stop {
        python::Stop::Deadline => format!(
            "The statement was stopped after {} seconds, the longest this emulator runs one \
             statement for.",
            STATEMENT_RUN_CEILING.as_secs()
        ),
        python::Stop::Steps => {
            "The statement reached the step budget this emulator gives one statement.".to_string()
        }
        python::Stop::Memory => {
            "The statement reached the memory ceiling this emulator gives one statement."
                .to_string()
        }
        python::Stop::Response => {
            "The statement answered with more than one statement may.".to_string()
        }
        python::Stop::HostCeiling => {
            "The statement took more host time than one run of one statement may take."
                .to_string()
        }
    }
}

/// `StatementOutput$ErrorValue` of a statement whose thread had still not answered when the
/// task that performs it gave up waiting ([`STATEMENT_HOST_WAIT`]).
pub const STATEMENT_UNANSWERED: &str =
    "The statement's own execution did not answer, so this emulator has no outcome for it.";

/// The module this emulator runs a statement's own code FROM, and the function in it the engine
/// calls.
const CELL_ENTRY_MODULE: &str = "__glue_session_statement";
/// See [`CELL_ENTRY_MODULE`].
const CELL_ENTRY_FUNCTION: &str = "run";

/// The Python source of that module: it runs the statement's code the way an interactive
/// runtime runs a cell — with the names a Glue session binds for it already bound, and the
/// value of a cell that is one expression rendered as that cell's output — and lets whatever
/// the code raises travel out to the engine, which is what reports it.
///
/// Nothing here is a bound or a containment: both are the engine's, in host code no cell can
/// reach.
const CELL_ENTRY_SOURCE: &str = r#"
"""Runs the code of one statement, as an interactive session's cell."""

from awsglue.context import GlueContext
from awsglue.job import Job
from pyspark.context import SparkContext

# What a cell's own traceback frames name the code it is running.
PATH = "<stdin>"


def _bound():
    """The names a cell finds already bound, as an interactive Glue session binds them."""
    context = SparkContext.getOrCreate()
    glue_context = GlueContext(context)
    return {
        "__name__": "__main__",
        "sc": context,
        "spark": glue_context.spark_session,
        "glueContext": glue_context,
        "job": Job(glue_context),
    }


def run(event, context):
    cell = _bound()
    source = event["code"]
    # A cell that is ONE expression ANSWERS that expression's value, the way an interactive
    # runtime answers one: it is evaluated and its value rendered as the cell's output. Any
    # other cell is a program, run as one, and what it reports is what it wrote itself.
    try:
        expression = compile(source, PATH, "eval")
    except SyntaxError:
        expression = None
    if expression is None:
        exec(compile(source, PATH, "exec"), cell)
        return {}
    answered = eval(expression, cell)
    if answered is not None:
        print(repr(answered))
    return {}
"#;
