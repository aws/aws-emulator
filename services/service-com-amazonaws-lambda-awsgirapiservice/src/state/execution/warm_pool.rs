// SPDX-License-Identifier: CC0-1.0

//! The EXECUTION ENVIRONMENTS this process keeps standing, and which invocation each may
//! serve.
//!
//! An invocation runs in an execution ENVIRONMENT: the engine of the function's own language
//! with this emulator's runtime installed and the handler's module loaded — an interpreter for
//! a `python3.x` function ([`super::python::Prepared`]), the JavaScript engine for a `nodejs*`
//! one ([`super::node::Standing`]). Building one is the INIT phase, and none of it depends on
//! the invocation that is served in it — which is why Lambda does not build one per call:
//! "After a Lambda function is invoked, Lambda maintains the execution environment for some
//! time in anticipation of another invocation", and an invocation an already-initialized
//! environment serves "skips the initialization phase". That reuse is also what makes the
//! global state a handler leaves behind — a client it created outside the handler, a cached
//! value, the work it left on its event loop — be there for the next invocation, which AWS
//! documents handlers may rely on.
//!
//! So this module holds the environments that are standing and idle, keyed by WHICH
//! invocations they may serve ([`EnvironmentKey`]): one function's one version, at one
//! revision of its configuration, in one Region, for one tenant, with one kind of
//! initialization — and, because the runtime is part of that key, an environment of one engine
//! never serves an invocation of the other. An invocation that finds no environment of its own
//! builds one; every other invocation is served by the environment already standing, and pays
//! none of that cost.
//!
//! An environment is a THREAD, because the engine and every object in it belong to the
//! thread they were built on: an invocation reaches one by being handed to its thread, which
//! answers what the run came to. Exactly one invocation is ever in flight in an environment —
//! an environment being used is OUT of this pool for as long as it is — so the invocations of
//! one function version are served in parallel by as many environments as arrive at once, as
//! many of them at an instant as the memory envelope admits runs ([`super::run_invocation`]).
//!
//! What a STANDING environment holds is bounded on its own, because that memory is held
//! outside the per-invocation envelope: an environment is kept only while the engine in it has
//! grown no further than the memory its function is configured for
//! ([`python::Prepared::grown_bytes`], [`node::Standing::grown_bytes`]) and has served fewer
//! than [`MAX_INVOCATIONS_PER_ENVIRONMENT`] invocations, and the environments standing idle are
//! held together to [`MAX_IDLE_ENVIRONMENTS`] of them and [`MAX_STANDING_BYTES`] of memory
//! between them, the longest-idle retired until both fit, and every one of them past
//! [`IDLE_LIFETIME`] retired with them — on the next operation the pool performs, because
//! nothing of this emulator polls a pool nobody is using.
//!
//! Three things end an environment, and each ends its THREAD, which tears its engine down and
//! gives that memory back. An invocation the HOST stopped leaves it mid-frame, so it is
//! not kept ([`reusable`]) — which is what AWS does with the environment of a run its timeout
//! ended. One of the bounds above being reached retires it. And so does the pool dropping it
//! to make room for another.
//!
//! Beside the initialized environments this module keeps a few BLANK ones ([`Blanks`]): the
//! half of an INTERPRETER environment that reads no function's configuration at all
//! ([`python::Blank`]) — the interpreter, the narrowing of its native surface and this
//! emulator's own runtime — stood up while a function is being PROVISIONED ([`provision`])
//! rather than on the path of its first invocation. AWS builds the environment of a function
//! while the function is not yet `Active`, so the first delivery to it does not pay for one;
//! here the same holds, because the first invocation ADOPTS a standing blank and pays its own
//! initialization alone. They are held under the same idle span and a count of their own
//! ([`MAX_STANDING_BLANKS`]), and an invocation that finds none standing stands one up itself,
//! so what this changes is who pays for that work and never what a run observes.

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::{channel, Receiver, SendError, Sender};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use super::super::durable_steps::DurableStepJournal;
use super::aws_api::AwsAccess;
use super::outcome::{Budget, Completion, Finished};
use super::package::PackageModule;
use super::run_clock::Clock;
use super::{node, python};

/// How long an environment nothing has used is kept standing.
///
/// `pub(super)` because the metadata session of an execution environment is retired on the same
/// span: an environment this pool no longer keeps standing is one whose session is no longer
/// that environment's ([`super::instance_metadata::session_of`]).
///
/// AWS states no span for this ("Lambda maintains the execution environment for some time"),
/// so it is this host's own: an idle environment holds its interpreter's memory, and this is
/// how long this process holds it in anticipation of the next invocation. It is far longer
/// than the gap between the calls of a client that is invoking a function steadily, so such a
/// client is served by warm environments throughout.
///
/// It is read when the pool is next USED — an invocation of the same key looking for an
/// environment, or any invocation returning one ([`retire_the_idle_span`]) — rather than by a
/// thread of this emulator watching a pool nobody is invoking against: what an environment
/// outstays this span for is the count and the memory bounds beside it, which hold whatever
/// happens next.
pub(super) const IDLE_LIFETIME: Duration = Duration::from_secs(60);
/// How many idle environments this process keeps at once.
///
/// One of the two bounds on what the standing environments hold of this host, beside
/// [`MAX_STANDING_BYTES`], and the longest-idle environment is retired to make room for a
/// newly returned one — so what the pool holds does not grow with the number of function
/// versions a client invokes.
///
/// It is a figure of its own rather than the count of runs the memory envelope admits at once
/// ([`super::MAX_CONCURRENT_RUNS`]): what is STANDING is held in anticipation of an invocation
/// nobody has made, so how much of this host that may take follows from what it can afford to
/// hold IDLE and not from how many handlers it serves at one instant. It is well above the
/// function versions one client invokes in a steady burst, so such a client is served by warm
/// environments throughout.
const MAX_IDLE_ENVIRONMENTS: usize = 16;

/// How much memory the idle environments of this process hold between them, at most.
///
/// The other bound, and the one that matters when the functions are large: an environment
/// that is STANDING holds its interpreter's memory outside the per-invocation envelope (the
/// share a run claims goes back when the invocation is answered), so the pool sums what each
/// of its environments has grown by ([`python::Prepared::grown_bytes`]) and retires the
/// longest-idle ones until the sum fits. It is stated here for the reason
/// [`MAX_IDLE_ENVIRONMENTS`] is: this is the memory this process holds for handlers it has
/// ALREADY run, which is not what the runs in flight may hold.
const MAX_STANDING_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// How many invocations one environment serves before it is recycled.
///
/// A standing environment is kept against the memory it has GROWN ([`serve_jobs`]), which is
/// the bound that matters; this is the backstop beside it, so that an environment serving a
/// function whose growth per invocation is below one reading is still recycled rather than
/// kept for the life of the process. AWS states no number for it either ([`IDLE_LIFETIME`]),
/// and it is far above the invocations a client makes in one steady burst.
const MAX_INVOCATIONS_PER_ENVIRONMENT: u64 = 10_000;

/// WHICH invocations one execution environment may serve.
///
/// Everything an environment is built from, so that no invocation is ever served by an
/// environment built for a different function, version, configuration, Region or tenant: the
/// handler's own module, the environment variables and the package are those of this one
/// function version at this one revision, and the revision changes whenever the code or the
/// configuration does. The kind of initialization is part of it too, because an environment
/// provisioned concurrency keeps initialized is not one an on-demand invocation is served by.
///
/// The TENANT is part of it because a reused environment holds what the guest left in it: an
/// invocation that names a tenant is served only in an environment built for that tenant, and
/// one that names none only in an environment built for none, so nothing a handler cached for
/// one tenant is reachable from another's invocation. The ACCOUNT is not a member because this
/// emulator serves one (`engine::mock_account_id`), so every invocation it admits is that
/// account's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EnvironmentKey {
    /// The function whose code the environment holds.
    pub function_name: String,
    /// The version of it the environment was built from.
    pub executed_version: String,
    /// The Region the invocation was addressed to.
    pub region: String,
    /// The revision that version's configuration was at: a function whose code or
    /// configuration has changed is at a new revision, so no environment built before the
    /// change can serve an invocation that arrives after it.
    pub code_revision: String,
    /// The runtime the environment's interpreter was built for.
    pub runtime: String,
    /// The handler it imported.
    pub handler: String,
    /// The memory the function is configured for.
    pub memory_size: i32,
    /// The architecture the runtime reports as its own.
    pub architecture: String,
    /// Which kind of execution environment this is, as the admission that let the first
    /// invocation in decided ([`super::super::execution_slot`]).
    pub initialization_type: &'static str,
    /// The `TenantId` the invocations it serves name, and `None` for the environments of the
    /// invocations that name none.
    pub tenant_id: Option<String>,
}

/// One invocation, as it travels to the environment that serves it.
///
/// Owned rather than borrowed, because it crosses to that environment's own thread. The
/// modules are shared rather than copied: they are read once per invocation and only an
/// environment being BUILT imports them.
pub struct Served {
    /// The deployment package's modules.
    pub modules: Arc<Vec<PackageModule>>,
    /// The `Handler` configured for the function.
    pub handler: String,
    /// The environment this invocation runs with.
    pub environment: BTreeMap<String, String>,
    /// The `Architectures` entry the function is configured for, which the run reports as
    /// the machine its execution environment has.
    pub architecture: String,
    /// The event payload, as the caller sent it.
    pub event: Option<String>,
    /// This invocation's request id.
    pub request_id: String,
    /// The ARN this invocation addressed.
    pub invoked_function_arn: String,
    /// The `ClientContext` the caller sent, decoded to its JSON text.
    pub client_context: Option<String>,
    /// The `TenantId` the caller sent.
    pub tenant_id: Option<String>,
    /// The Amazon Cognito identity this invocation was made BY
    /// ([`super::super::caller_cognito_identity`]), and `None` for one made by a credential no
    /// identity pool vended.
    pub cognito_identity: Option<crate::state::InvocationIdentity>,
    /// What this invocation may spend.
    pub budget: Budget,
    /// How its handler reaches the rest of this emulator.
    pub aws: Option<AwsAccess>,
    /// Where it reports the STEPS of the durable execution it is the run of
    /// ([`super::super::durable_steps`]), and `None` for an invocation that is no durable
    /// execution's run.
    pub durable: Option<DurableStepJournal>,
    /// The FILE SYSTEMS its execution environment mounts
    /// ([`super::super::file_system_mounts`]), resolved for this invocation.
    pub file_systems: Vec<crate::state::FileSystemMount>,
}

impl Served {
    /// This invocation as the run the interpreter is handed.
    fn as_run(&self) -> python::Run<'_> {
        python::Run {
            modules: &self.modules,
            handler: &self.handler,
            environment: &self.environment,
            architecture: &self.architecture,
            event: self.event.as_deref(),
            request_id: &self.request_id,
            invoked_function_arn: &self.invoked_function_arn,
            client_context: self.client_context.as_deref(),
            tenant_id: self.tenant_id.as_deref(),
            cognito_identity: self.cognito_identity.as_ref(),
            budget: self.budget,
            aws: self.aws.as_ref(),
            durable: self.durable.as_ref(),
            file_systems: &self.file_systems,
        }
    }

    /// This invocation as the run the JAVASCRIPT engine is handed.
    ///
    /// The same invocation, read by the other engine: what it carries beyond the python run
    /// above is the release line the runtime reports and the engine's own allocation ceiling,
    /// both of which are the ENVIRONMENT's and are therefore read off the key the environment
    /// is held under ([`EnvironmentKey`]) rather than stated twice. What it does not carry is
    /// the durable journal and the file-system mounts, which are no part of a node run
    /// ([`node::Run`]).
    fn as_node_run<'a>(&'a self, key: &'a EnvironmentKey) -> node::Run<'a> {
        node::Run {
            modules: &self.modules,
            handler: &self.handler,
            runtime: &key.runtime,
            architecture: &self.architecture,
            environment: &self.environment,
            event: self.event.as_deref(),
            request_id: &self.request_id,
            invoked_function_arn: &self.invoked_function_arn,
            client_context: self.client_context.as_deref(),
            tenant_id: self.tenant_id.as_deref(),
            cognito_identity: self.cognito_identity.as_ref(),
            budget: self.budget,
            heap_bytes: super::run_heap_bytes(key.memory_size),
            aws: self.aws.as_ref(),
        }
    }
}

/// Serve `served` in an environment of `key`: the one already standing when there is one,
/// and a new one when there is not.
///
/// An environment taken from the pool whose thread has gone — which it can only have done by
/// panicking — takes no invocation, and that invocation is then served by a NEW environment
/// rather than answered a failure, so a lost environment is invisible to the caller.
///
/// The caller's own thread waits here for the run's answer, exactly as it waited for the
/// interpreter it used to build itself; the ceiling on that wait and the accounting of a run
/// the caller gave up on are [`super::run_invocation`]'s, whichever environment served it.
#[must_use]
pub fn serve(key: &EnvironmentKey, served: Served) -> Finished {
    let mut served = served;
    // Two attempts at most: only an environment that was already standing can be lost this
    // way, so the second attempt builds one and has nothing to lose.
    for standing_first in [true, false] {
        let standing = match standing_first
            .then(|| checkout(key))
            .flatten()
            .or_else(|| start(key))
        {
            Some(standing) => standing,
            None => {
                return reported(
                    "an execution environment for this invocation could not be started"
                        .to_string(),
                )
            }
        };
        let (answered, answer) = channel();
        if let Err(SendError(job)) = standing.jobs.send(Job {
            served,
            answer: answered,
        }) {
            served = job.served;
            continue;
        }
        return match answer.recv() {
            Ok(Answered {
                finished,
                reusable,
                grown,
            }) => {
                if reusable {
                    give_back(key, standing, grown);
                }
                finished
            }
            // The environment's thread ended without answering. Nothing of the run can be
            // reported, and the environment is not kept: this handle is dropped here.
            Err(_) => reported(
                "the execution environment ended without reporting the invocation".to_string(),
            ),
        };
    }
    reported("the execution environment did not take the invocation".to_string())
}

/// What this module reports for a run that did not happen at all: never a result, only the
/// internal failure it is ([`Completion::Internal`]).
fn reported(why: String) -> Finished {
    Finished {
        completion: Completion::Internal(why),
        log: String::new(),
        log_records: Vec::new(),
        init_records: Vec::new(),
        // Nothing was initialized, because nothing ran at all.
        initialized: false,
        init_duration: None,
        log_stream: None,
        elapsed: Duration::ZERO,
        // Nothing ran, so no handler asked to wait for anything and no span of a run is still
        // to pass.
        unwaited: Duration::ZERO,
        // Nothing ran, so no reading of a run's memory was taken.
        max_memory_bytes: None,
    }
}

/// A standing environment, reached by handing it an invocation.
struct Standing {
    /// Where its thread takes invocations. Dropping this ends that thread, which finalizes
    /// its interpreter.
    jobs: Sender<Job>,
}

/// One invocation handed to an environment, with where its answer goes.
struct Job {
    /// The invocation to serve.
    served: Served,
    /// Where what the run came to is sent back.
    answer: Sender<Answered>,
}

/// What one invocation came to, and whether the environment that served it may serve
/// another.
struct Answered {
    /// What the run came to.
    finished: Finished,
    /// Whether the environment is still one an invocation can be served in.
    reusable: bool,
    /// What the environment has grown by since it was built
    /// ([`python::Prepared::grown_bytes`]), which is what the pool holds it against.
    grown: u64,
}

/// Stand an execution environment up for a function that is being PROVISIONED, so the first
/// invocation of that function is the handler's own run and not the build of an interpreter.
///
/// What is stood up is the half of an environment that reads no function's configuration
/// ([`python::Blank`]), which is by far the larger half: the interpreter, the host surface it
/// is narrowed to and this emulator's own runtime source. The invocation that arrives later
/// ADOPTS it — the function's own configuration is applied to it and the handler's module is
/// imported then, which is the INIT phase and the `Init Duration` of a cold start.
///
/// Called from the operations that provision a function — creating one, and changing the code
/// or the configuration of one — because that is the window AWS builds an environment in: the
/// function reports a non-terminal `State` or `LastUpdateStatus` while it is open, and a
/// caller that polls until it closes has an environment waiting for its first invocation.
/// Only a function whose runtime runs on the interpreter this module stands up is provisioned
/// for; a function of any other runtime has no environment of this kind.
///
/// It never BLOCKS the caller: the environment is built on a thread of its own, and an
/// operation that asks for one is answered whether or not it is standing yet.
pub fn provision(runtime: Option<&str>) {
    if super::engine_for(runtime) != Some(super::Engine::Python) {
        return;
    }
    let asked = {
        let mut standing = blanks().lock().unwrap_or_else(PoisonError::into_inner);
        asked_for(&mut standing)
    };
    if asked {
        stand_up_blank();
    }
}

/// Whether one more blank environment is asked for, and claimed when it is: [`provision`]'s
/// whole decision, with nothing of the process in it.
///
/// A blank past [`IDLE_LIFETIME`] is retired first, for the reason the initialized ones are
/// ([`retire_the_idle_span`]): the span is enforced wherever the pool is next used.
fn asked_for(blanks: &mut Blanks) -> bool {
    retire_the_blank_idle_span(blanks);
    // As many as this process keeps are standing or on their way, so this provisioning asks
    // for none: what is already standing serves the next invocation that needs an
    // environment, whichever function it is of.
    if blanks.waiting.len() + blanks.building >= MAX_STANDING_BLANKS {
        return false;
    }
    // Claimed from HERE rather than when the build finishes, so provisionings at the same
    // instant never stand more up than this process keeps.
    blanks.building += 1;
    true
}

/// Start the thread of one BLANK environment: it stands the function-independent half of an
/// environment up and parks itself for the first invocation that needs one.
///
/// The claim this provisioning made on the count of environments being stood up
/// ([`Blanks::building`]) is released by a GUARD rather than by each arm, so it is given back
/// however the build ends — the environment standing, the build failing, or the thread
/// unwinding out of it — and a provisioning that came to nothing can never leave the count
/// claimed for a build that is no longer happening.
fn stand_up_blank() {
    let (jobs, incoming) = channel();
    let spawned = ::std::thread::Builder::new()
        .name("lambda-execution-environment".to_string())
        .stack_size(super::RUN_STACK_BYTES)
        .spawn(move || {
            let claim = Building;
            let clock = Clock::host();
            match python::Blank::stand_up(&clock) {
                // Parked only once it really is standing, so a build that failed hands
                // nothing to an invocation: the handle goes into the pool and this thread
                // keeps the environment itself, which belongs to this thread.
                Ok(blank) => {
                    let held = parked(Standing { jobs });
                    // The build is over, whether or not the pool had room for what it built.
                    drop(claim);
                    match held {
                        Parked::Waiting => serve_in(incoming, blank, &clock, None),
                        // As many are standing as this process keeps, so this one is not: it
                        // is torn down here rather than held outside that bound.
                        Parked::Refused => blank.discard(),
                    }
                }
                Err(why) => {
                    ::tracing::debug!(
                        why,
                        "an execution environment this emulator was standing up ahead of an \
                         invocation could not be stood up, so the first invocation that needs \
                         one builds its own"
                    );
                }
            }
        });
    if spawned.is_err() {
        // No thread took the claim, so it is given back here.
        building_ended();
    }
}

/// One blank environment being stood up, as the pool counts it: the claim
/// [`asked_for`] made, given back when this is dropped.
struct Building;

impl Drop for Building {
    fn drop(&mut self) {
        building_ended();
    }
}

/// Start an environment for an invocation that is waiting for one NOW: a thread that stands
/// the environment up, lets that invocation adopt it and then serves invocation after
/// invocation.
///
/// The handle is answered at once rather than when the environment stands, so the invocation
/// is already on its way to the thread while the thread is building: what it waits for is the
/// build and its own initialization, which is what an invocation that finds nothing standing
/// pays.
fn building() -> Option<Standing> {
    let (jobs, incoming) = channel();
    ::std::thread::Builder::new()
        .name("lambda-execution-environment".to_string())
        .stack_size(super::RUN_STACK_BYTES)
        .spawn(move || serve_jobs(incoming))
        .ok()?;
    Some(Standing { jobs })
}

/// An environment for the invocation on its way to it: one the engine that serves this
/// invocation's runtime stands on.
///
/// The PYTHON engine has blank environments standing ahead of an invocation ([`Blanks`]), so
/// one of those is adopted where there is one, and a thread that builds one is started where
/// there is not. The JAVASCRIPT engine has no blank half to adopt — an environment of it is
/// built from the function's own package ([`node::Standing::stand_up`]) — so a thread is
/// started that builds one for this invocation and then keeps it standing.
fn start(key: &EnvironmentKey) -> Option<Standing> {
    match super::engine_for(Some(&key.runtime)) {
        Some(super::Engine::Node) => node_building(key.clone()),
        _ => adopted_blank().or_else(building),
    }
}

/// Start the thread of a NODE environment for an invocation that is waiting for one now: it
/// takes that invocation, builds the environment its package needs and then serves invocation
/// after invocation in it.
fn node_building(key: EnvironmentKey) -> Option<Standing> {
    let (jobs, incoming) = channel();
    ::std::thread::Builder::new()
        .name("lambda-execution-environment".to_string())
        .stack_size(super::RUN_STACK_BYTES)
        .spawn(move || serve_node_jobs(incoming, &key))
        .ok()?;
    Some(Standing { jobs })
}

/// One JAVASCRIPT execution environment's whole life, on its own thread.
///
/// The environment is built from the FIRST invocation's own package — there is no half of one
/// that reads no function's configuration here — and every invocation that follows is served
/// in it, which is what makes the state a handler left behind, the modules it loaded and the
/// timers it scheduled and did not wait for be there for the next invocation
/// ([`node::Standing`]).
fn serve_node_jobs(jobs: Receiver<Job>, key: &EnvironmentKey) {
    let building = Instant::now();
    let clock = Clock::host();
    let Ok(first) = jobs.recv() else {
        // Nothing will be served here: the pool dropped this handle, or this process is going
        // down. Nothing was built, so there is nothing to tear down.
        return;
    };
    // The log STREAM this execution environment writes to: one stream per environment, which
    // is what makes the invocations a standing environment serves share a stream and a new
    // environment open one of its own.
    let log_stream = first
        .served
        .environment
        .get(super::LOG_STREAM_VARIABLE)
        .cloned();
    let mut budget = first.served.budget;
    let standing = match node::Standing::stand_up(&first.served.as_node_run(key), &clock) {
        Ok(standing) => standing,
        Err(failed) => {
            // An environment that could not be stood up is answered to the invocation that
            // asked for one, and none is kept.
            let mut finished = *failed;
            finished.elapsed = building.elapsed();
            finished.log_stream = log_stream.clone();
            let _ = first.answer.send(Answered {
                finished,
                reusable: false,
                grown: 0,
            });
            return;
        }
    };
    let mut cold = Some(building);
    let mut job = first;
    let mut served = 0u64;
    loop {
        served = served.saturating_add(1);
        // Every invocation served here reports the stream of the environment serving it,
        // whichever stream its own caller composed.
        if let Some(stream) = log_stream.clone() {
            job.served
                .environment
                .insert(super::LOG_STREAM_VARIABLE.to_string(), stream);
        }
        let mut finished = standing.serve(&job.served.as_node_run(key), &clock);
        finished.log_stream = log_stream.clone();
        // The invocation the environment was BUILT for waited for that build as well as for
        // its own run, so that is what its duration is, and what the build itself took is what
        // its report states as its `Init Duration`. Every later invocation waited for its own
        // run alone, which is the whole point of the environment still standing.
        if let Some(initializing) = cold.take() {
            let whole = initializing.elapsed();
            finished.init_duration = Some(whole.saturating_sub(finished.elapsed));
            finished.elapsed = whole;
        }
        // Whether this environment may stand: the invocation it just served has to have
        // COMPLETED, and the engine in it has to be within the memory its function is
        // configured for and within the invocations one environment serves — the same two
        // bounds the interpreter environments are kept against ([`serve_in`]).
        let grown = standing.grown_bytes();
        let within_memory = grown <= budget.memory_bytes;
        let keep = reusable(&finished.completion)
            && within_memory
            && served < MAX_INVOCATIONS_PER_ENVIRONMENT;
        if !within_memory {
            ::tracing::debug!(
                grown_bytes = grown,
                configured_bytes = budget.memory_bytes,
                served,
                "an execution environment of this emulator has grown past the memory its \
                 function is configured for, so it was retired rather than kept standing"
            );
        }
        let _ = job.answer.send(Answered {
            finished,
            reusable: keep,
            grown,
        });
        if !keep {
            break;
        }
        match jobs.recv() {
            Ok(next) => {
                budget = next.served.budget;
                job = next;
            }
            // The pool dropped this environment: its idle span ran out, it was retired to
            // make room, or this process is going down.
            Err(_) => break,
        }
    }
    standing.discard();
}

/// One execution environment's whole life, on its own thread, for an invocation that is
/// already on its way to it.
fn serve_jobs(jobs: Receiver<Job>) {
    // The build begins before the invocation is taken out of the channel, because this
    // invocation waits for it: it found no environment standing, so standing one up is on its
    // path ([`python::Prepared::start`]).
    let building = Instant::now();
    let clock = Clock::host();
    match python::Blank::stand_up(&clock) {
        Ok(blank) => serve_in(jobs, blank, &clock, Some(building)),
        Err(why) => {
            // No environment could be stood up at all. The invocation on its way here is
            // answered that, because nothing of it ran.
            if let Ok(first) = jobs.recv() {
                let _ = first.answer.send(Answered {
                    finished: reported(why),
                    reusable: false,
                    grown: 0,
                });
            }
        }
    }
}

/// One execution environment's whole life once its blank half is STANDING: the first
/// invocation that arrives adopts it, and it then serves invocation after invocation.
///
/// `building` is when the build of that half began, for an invocation that waited for it, and
/// `None` for an environment that was already standing when the invocation arrived — which is
/// what decides how much of the wait the invocation that adopts it is reported to have spent.
fn serve_in(jobs: Receiver<Job>, blank: python::Blank, clock: &Clock, building: Option<Instant>) {
    let Ok(first) = jobs.recv() else {
        // Nothing will adopt this environment: the pool dropped it, or this process is going
        // down. It is torn down rather than left standing.
        blank.discard();
        return;
    };
    let initializing = building.unwrap_or_else(Instant::now);
    // The log STREAM this execution environment writes to: one stream per environment, which
    // is what makes the invocations a standing environment serves share a stream and a new
    // environment open one of its own. It is the stream the invocation that ADOPTED the
    // environment was composed with, and every later invocation served here is told the same
    // one — so what the runtime reports as its stream and what the delivery writes to are the
    // environment's own ([`super::log_stream_name`]).
    let log_stream = first
        .served
        .environment
        .get(super::LOG_STREAM_VARIABLE)
        .cloned();
    // What the environment is finally torn down against: the budget of the LAST invocation it
    // served, so a guest `atexit` callback is bounded like the handlers were.
    let mut budget = first.served.budget;
    let prepared = match blank.adopt(&first.served.as_run(), clock) {
        Ok(prepared) => prepared,
        Err(failed) => {
            // An initialization that failed is answered to the invocation that asked for it,
            // and no environment is kept.
            let mut finished = *failed;
            finished.elapsed = initializing.elapsed();
            finished.log_stream = log_stream.clone();
            let _ = first.answer.send(Answered {
                finished,
                reusable: false,
                grown: 0,
            });
            return;
        }
    };
    let mut cold = Some(initializing);
    let mut job = first;
    let mut served = 0u64;
    loop {
        served = served.saturating_add(1);
        // Every invocation served here reports the stream of the environment serving it,
        // whichever stream its own caller composed.
        if let Some(stream) = log_stream.clone() {
            job.served
                .environment
                .insert(super::LOG_STREAM_VARIABLE.to_string(), stream);
        }
        let mut finished = prepared.invoke(&job.served.as_run(), clock);
        finished.log_stream = log_stream.clone();
        // The invocation that ADOPTED the environment waited for its own initialization in it
        // as well as for its own run, so that is what its duration is. Every later invocation
        // waited for its own run alone, which is the whole point of the environment still
        // standing.
        if let Some(initializing) = cold.take() {
            let whole = initializing.elapsed();
            // What the INITIALIZATION itself took: the whole wait, less the handler's own run
            // — which is the span the report of a cold start states as its `Init Duration`.
            finished.init_duration = Some(whole.saturating_sub(finished.elapsed));
            finished.elapsed = whole;
        }
        // Whether this environment may stand: the invocation it just served has to have
        // COMPLETED, and the environment itself has to be within what a standing one may hold
        // of this host — the memory its interpreter has grown by since it was built, against
        // the memory its function is configured for (`budget.memory_bytes`, which is
        // [`super::run_heap_bytes`] of that `MemorySize`), and the invocations it has served.
        // Both are measured HERE, on the environment's own thread, because that thread's
        // memory is what is held once the invocation has been answered and the per-invocation
        // envelope share has gone back.
        let grown = prepared.grown_bytes();
        let within_memory = grown <= budget.memory_bytes;
        let keep = reusable(&finished.completion)
            && within_memory
            && served < MAX_INVOCATIONS_PER_ENVIRONMENT;
        if !within_memory {
            ::tracing::debug!(
                grown_bytes = grown,
                configured_bytes = budget.memory_bytes,
                served,
                "an execution environment of this emulator has grown past the memory its \
                 function is configured for, so it was retired rather than kept standing"
            );
        }
        let _ = job.answer.send(Answered {
            finished,
            reusable: keep,
            grown,
        });
        if !keep {
            break;
        }
        match jobs.recv() {
            Ok(next) => {
                budget = next.served.budget;
                job = next;
            }
            // The pool dropped this environment: its idle span ran out, it was retired to
            // make room, or this process is going down.
            Err(_) => break,
        }
    }
    prepared.finalize(budget, clock);
}

/// Whether the environment that served an invocation may serve another.
///
/// An invocation that COMPLETED leaves its environment standing, whether its handler
/// returned or raised — which is why what a handler left in its module globals is there for
/// the next invocation. An invocation the host STOPPED does not: a run its deadline, its step
/// budget, a memory ceiling or the response cap ended leaves the interpreter part way through
/// a frame, and a run that could not be driven at all leaves an environment nothing can be
/// said about, so neither is kept.
fn reusable(completion: &Completion) -> bool {
    match completion {
        Completion::Returned(_) | Completion::Streamed { .. } | Completion::Raised { .. } => true,
        Completion::Stopped(_) | Completion::Internal(_) => false,
    }
}

/// An environment standing idle, and since when.
struct Idle {
    /// Where its thread takes invocations.
    jobs: Sender<Job>,
    /// When it last finished serving one.
    since: Instant,
    /// What the environment had grown by when it was returned, which is what the pool holds
    /// against [`MAX_STANDING_BYTES`].
    grown: u64,
}

/// The idle environments of this process.
#[derive(Default)]
struct Pool {
    /// The idle environments of each key, in the order they were returned — so the last of a
    /// list is the one that was used most recently.
    idle: HashMap<EnvironmentKey, Vec<Idle>>,
    /// How many idle environments are held across every key.
    held: usize,
    /// What the idle environments hold of this host between them: the sum of what each has
    /// grown by ([`Idle::grown`]).
    held_bytes: u64,
}

/// The one pool of this process.
fn pool() -> &'static Mutex<Pool> {
    static POOL: OnceLock<Mutex<Pool>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(Pool::default()))
}

/// Take the environment of `key` that was used most recently, when one is standing.
///
/// An environment that has been idle for longer than [`IDLE_LIFETIME`] is not served from:
/// it is dropped here, which ends its thread. The list is in return order, so an entry older
/// than that span has only older ones behind it.
fn checkout(key: &EnvironmentKey) -> Option<Standing> {
    let mut pool = pool().lock().unwrap_or_else(PoisonError::into_inner);
    taken_from(&mut pool, key)
}

/// [`checkout`], on the pool handed in: the whole decision, with nothing of the process in
/// it.
fn taken_from(pool: &mut Pool, key: &EnvironmentKey) -> Option<Standing> {
    let mut idle = pool.idle.remove(key)?;
    let mut taken = None;
    let mut left = 0;
    let mut freed = 0;
    while let Some(entry) = idle.pop() {
        left += 1;
        freed += entry.grown;
        if entry.since.elapsed() < IDLE_LIFETIME {
            taken = Some(Standing { jobs: entry.jobs });
            break;
        }
    }
    if taken.is_none() {
        // Every entry behind the freshest one is older still, so none of them is kept.
        left += idle.len();
        freed += idle.iter().map(|entry| entry.grown).sum::<u64>();
        idle.clear();
    }
    pool.held = pool.held.saturating_sub(left);
    pool.held_bytes = pool.held_bytes.saturating_sub(freed);
    if !idle.is_empty() {
        pool.idle.insert(key.clone(), idle);
    }
    taken
}

/// Keep `standing`, which has grown by `grown` since it was built, for the next invocation of
/// `key`.
///
/// Every key's idle environments are pruned of the ones past [`IDLE_LIFETIME`] first, so that
/// span is enforced across the whole pool rather than only where the next invocation of the
/// same key happens to look. What is left then has to fit BOTH bounds on what the standing
/// environments hold of this host — [`MAX_IDLE_ENVIRONMENTS`] of them, and
/// [`MAX_STANDING_BYTES`] of memory between them — and the longest-idle environment is retired
/// until it does; an environment that does not fit even an empty pool is dropped instead of
/// kept. Each of those drops ends an environment's thread, which finalizes its interpreter and
/// gives that memory back.
fn give_back(key: &EnvironmentKey, standing: Standing, grown: u64) {
    let mut pool = pool().lock().unwrap_or_else(PoisonError::into_inner);
    kept_in(&mut pool, key, standing, grown);
}

/// [`give_back`], on the pool handed in: the whole decision, with nothing of the process in
/// it.
fn kept_in(pool: &mut Pool, key: &EnvironmentKey, standing: Standing, grown: u64) {
    retire_the_idle_span(pool);
    while pool.held >= MAX_IDLE_ENVIRONMENTS
        || pool.held_bytes.saturating_add(grown) > MAX_STANDING_BYTES
    {
        if !retire_longest_idle(pool) {
            // Nothing is left to retire: this environment is the one that does not fit, so it
            // is dropped here rather than kept outside the bounds above.
            return;
        }
    }
    pool.held += 1;
    pool.held_bytes = pool.held_bytes.saturating_add(grown);
    pool.idle.entry(key.clone()).or_default().push(Idle {
        jobs: standing.jobs,
        since: Instant::now(),
        grown,
    });
}

/// Drop every idle environment of every key that has stood unused for [`IDLE_LIFETIME`].
fn retire_the_idle_span(pool: &mut Pool) {
    let mut retired = 0;
    let mut freed = 0;
    for idle in pool.idle.values_mut() {
        let before = idle.len();
        freed += idle
            .iter()
            .filter(|entry| entry.since.elapsed() >= IDLE_LIFETIME)
            .map(|entry| entry.grown)
            .sum::<u64>();
        idle.retain(|entry| entry.since.elapsed() < IDLE_LIFETIME);
        retired += before - idle.len();
    }
    pool.idle.retain(|_, idle| !idle.is_empty());
    pool.held = pool.held.saturating_sub(retired);
    pool.held_bytes = pool.held_bytes.saturating_sub(freed);
}

/// Drop the environment that has been idle longest, and report whether there was one.
fn retire_longest_idle(pool: &mut Pool) -> bool {
    let Some(key) = pool
        .idle
        .iter()
        .filter_map(|(key, idle)| idle.first().map(|entry| (entry.since, key.clone())))
        .min_by_key(|(since, _)| *since)
        .map(|(_, key)| key)
    else {
        return false;
    };
    let Some(idle) = pool.idle.get_mut(&key) else {
        return false;
    };
    // The FRONT of a key's list is its longest-idle environment.
    if idle.is_empty() {
        return false;
    }
    let retired = idle.remove(0);
    if idle.is_empty() {
        pool.idle.remove(&key);
    }
    pool.held = pool.held.saturating_sub(1);
    pool.held_bytes = pool.held_bytes.saturating_sub(retired.grown);
    true
}

/// The BLANK environments of this process: the ones standing with no function in them yet.
///
/// They are held apart from the initialized ones above because WHICH invocation each may
/// serve is the one thing they do not decide: a blank environment has adopted no function, so
/// the next invocation that needs an environment of any key at all is served in it.
#[derive(Default)]
struct Blanks {
    /// The blank environments standing in wait, in the order they were stood up — so the last
    /// of the list is the freshest.
    waiting: Vec<Waiting>,
    /// How many are being stood up right now, counted from the instant one is asked for
    /// ([`provision`]) so that provisionings at the same instant never stand more up than
    /// this process keeps.
    building: usize,
    /// How many standing blanks invocations have ADOPTED since this process started
    /// ([`adopted_blanks`]).
    adopted: u64,
}

/// One blank environment standing in wait, and since when.
struct Waiting {
    /// Where its thread takes the invocation that adopts it. Dropping this ends that thread,
    /// which tears the environment down ([`python::Blank::discard`]).
    jobs: Sender<Job>,
    /// When it finished standing up.
    since: Instant,
}

/// How many execution environments this process stands up AHEAD of the invocations that adopt
/// them.
///
/// A blank environment holds an interpreter of this host whether or not an invocation ever
/// comes for it, so this is deliberately small: it is what a function being provisioned asks
/// for, and the next function provisioned while they stand asks for none. AWS states no
/// number for what it keeps warm for a function nobody has invoked, so this is this host's
/// own — enough that the functions a client creates in one burst each find an environment
/// waiting, and far below what the initialized environments beside them may hold
/// ([`MAX_IDLE_ENVIRONMENTS`]).
const MAX_STANDING_BLANKS: usize = 2;

/// The blank environments of this process.
fn blanks() -> &'static Mutex<Blanks> {
    static BLANKS: OnceLock<Mutex<Blanks>> = OnceLock::new();
    BLANKS.get_or_init(|| Mutex::new(Blanks::default()))
}

/// How many blank environments are STANDING in this process right now: the ones a function's
/// provisioning stood up and no invocation has adopted yet.
///
/// A reading of this module's own pool, like [`super::abandoned_runs`] is of its run
/// accounting: it answers what is standing and changes nothing.
#[must_use]
pub fn standing_blanks() -> usize {
    blanks()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .waiting
        .len()
}

/// How many blank environments invocations of this process have ADOPTED since it started.
///
/// Each one is an invocation that found an environment standing and therefore paid none of
/// the build it would otherwise have waited for. The count never falls, for the same reason
/// [`standing_blanks`] exists: it is a reading, and reading it changes nothing.
#[must_use]
pub fn adopted_blanks() -> u64 {
    blanks()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .adopted
}

/// Whether a blank environment that has just been stood up was taken into the pool.
enum Parked {
    /// It is in the pool, waiting for the invocation that adopts it.
    Waiting,
    /// As many are standing as this process keeps, so this one is not held at all.
    Refused,
}

/// Hold `standing`, which has just finished standing up, for the first invocation that needs
/// an environment.
fn parked(standing: Standing) -> Parked {
    let mut blanks = blanks().lock().unwrap_or_else(PoisonError::into_inner);
    parked_in(&mut blanks, standing)
}

/// [`parked`], on the pool handed in: the whole decision, with nothing of the process in it.
///
/// Every blank past [`IDLE_LIFETIME`] is retired first, for the reason the initialized ones
/// are ([`retire_the_idle_span`]). The claim the build held is NOT given back here: it is the
/// caller's guard that gives it back ([`Building`]), so a build that never reached this point
/// gives it back too.
fn parked_in(blanks: &mut Blanks, standing: Standing) -> Parked {
    retire_the_blank_idle_span(blanks);
    if blanks.waiting.len() >= MAX_STANDING_BLANKS {
        return Parked::Refused;
    }
    blanks.waiting.push(Waiting {
        jobs: standing.jobs,
        since: Instant::now(),
    });
    Parked::Waiting
}

/// Give back the claim one build held on the count of environments being stood up: the
/// thread could not be started, or the build on it is over ([`Building`]).
fn building_ended() {
    let mut blanks = blanks().lock().unwrap_or_else(PoisonError::into_inner);
    ended_in(&mut blanks);
}

/// [`building_ended`], on the pool handed in.
fn ended_in(blanks: &mut Blanks) {
    blanks.building = blanks.building.saturating_sub(1);
}

/// Take a blank environment for an invocation that needs one, when one is standing.
fn adopted_blank() -> Option<Standing> {
    let mut blanks = blanks().lock().unwrap_or_else(PoisonError::into_inner);
    taken_blank_from(&mut blanks)
}

/// [`adopted_blank`], on the pool handed in: the whole decision, with nothing of the process
/// in it.
///
/// The freshest is taken and every one behind it that has stood past [`IDLE_LIFETIME`] is
/// dropped, which ends its thread — the same reading [`taken_from`] serves an initialized
/// environment by.
fn taken_blank_from(blanks: &mut Blanks) -> Option<Standing> {
    while let Some(waiting) = blanks.waiting.pop() {
        if waiting.since.elapsed() < IDLE_LIFETIME {
            blanks.adopted = blanks.adopted.saturating_add(1);
            return Some(Standing {
                jobs: waiting.jobs,
            });
        }
    }
    None
}

/// Drop every blank environment that has stood unused for [`IDLE_LIFETIME`].
fn retire_the_blank_idle_span(blanks: &mut Blanks) {
    blanks
        .waiting
        .retain(|waiting| waiting.since.elapsed() < IDLE_LIFETIME);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key naming one function version for one tenant, differing from every other by
    /// `revision` and `tenant`.
    fn key_for(revision: &str, tenant: Option<&str>) -> EnvironmentKey {
        EnvironmentKey {
            function_name: "probe".to_string(),
            executed_version: "$LATEST".to_string(),
            region: "us-east-1".to_string(),
            code_revision: revision.to_string(),
            runtime: "python3.12".to_string(),
            handler: "lambda_function.lambda_handler".to_string(),
            memory_size: 128,
            architecture: "x86_64".to_string(),
            initialization_type: "on-demand",
            tenant_id: tenant.map(str::to_string),
        }
    }

    /// That same key for an invocation that names no tenant.
    fn key(revision: &str) -> EnvironmentKey {
        key_for(revision, None)
    }

    /// What an environment may be taken for: its own key and nothing else. The rows are one
    /// per member of the key a caller or a change to the function can vary.
    #[test]
    fn an_environment_is_held_for_its_own_key_alone() {
        let held = key_for("one", Some("tenant-a"));
        let cases: &[(&str, EnvironmentKey)] = &[
            ("another revision of the same function", key_for("two", Some("tenant-a"))),
            ("another tenant", key_for("one", Some("tenant-b"))),
            ("no tenant at all", key_for("one", None)),
            (
                "another version",
                EnvironmentKey {
                    executed_version: "7".to_string(),
                    ..held.clone()
                },
            ),
            (
                "another Region",
                EnvironmentKey {
                    region: "eu-west-1".to_string(),
                    ..held.clone()
                },
            ),
            (
                "another function of the same revision",
                EnvironmentKey {
                    function_name: "other".to_string(),
                    ..held.clone()
                },
            ),
            (
                "another handler",
                EnvironmentKey {
                    handler: "lambda_function.other".to_string(),
                    ..held.clone()
                },
            ),
            (
                "another memory size",
                EnvironmentKey {
                    memory_size: 512,
                    ..held.clone()
                },
            ),
            (
                "an environment provisioned concurrency keeps initialized",
                EnvironmentKey {
                    initialization_type: "provisioned-concurrency",
                    ..held.clone()
                },
            ),
        ];
        for (case, other) in cases {
            let mut pool = Pool::default();
            let (standing, _incoming) = environment();
            kept_in(&mut pool, &held, standing, 0);
            assert!(
                taken_from(&mut pool, other).is_none(),
                "case {case}: no environment may be served to it"
            );
            assert!(
                taken_from(&mut pool, &held).is_some(),
                "case {case}: the environment must be served to the key it was returned under"
            );
        }
    }

    /// An environment, with the end of the channel its thread would be waiting on.
    fn environment() -> (Standing, Receiver<Job>) {
        let (jobs, incoming) = channel::<Job>();
        (Standing { jobs }, incoming)
    }

    /// What the pool does with the environments it is handed: it serves one back to its own
    /// key, serves none that has stood past the idle span, and keeps no more standing than
    /// its two bounds allow — each environment it does not keep having its channel closed,
    /// which is what ends that environment's thread.
    #[test]
    fn the_pool_serves_what_it_holds_and_retires_what_it_may_not_hold() {
        let mut pool = Pool::default();
        let (standing, incoming) = environment();
        kept_in(&mut pool, &key("one"), standing, 1024);
        assert_eq!(
            (pool.held, pool.held_bytes),
            (1, 1024),
            "case one environment returned: expected it held and accounted for, got {} and {}",
            pool.held,
            pool.held_bytes
        );
        let taken = taken_from(&mut pool, &key("one"));
        assert!(
            taken.is_some(),
            "case the next invocation of its key: the environment must be served to it"
        );
        assert_eq!(
            (pool.held, pool.held_bytes),
            (0, 0),
            "case served out: the pool holds it no longer, got {} and {}",
            pool.held,
            pool.held_bytes
        );
        drop(taken);
        assert!(
            incoming.recv().is_err(),
            "case served out and then dropped: its thread must see its channel closed"
        );

        // An environment that has stood past the idle span is not served from: it is retired,
        // and the invocation that looked for it builds its own.
        let (standing, incoming) = environment();
        kept_in(&mut pool, &key("one"), standing, 2048);
        for idle in pool.idle.values_mut() {
            for entry in idle.iter_mut() {
                entry.since -= IDLE_LIFETIME;
            }
        }
        assert!(
            taken_from(&mut pool, &key("one")).is_none(),
            "case an environment past its idle span: it must not be served from"
        );
        assert_eq!(
            (pool.held, pool.held_bytes),
            (0, 0),
            "case an environment past its idle span: it must be accounted for as retired, got \
             {} and {}",
            pool.held,
            pool.held_bytes
        );
        assert!(
            incoming.recv().is_err(),
            "case an environment past its idle span: its thread must see its channel closed"
        );

        // Past the COUNT the pool keeps, the longest-idle environment is retired to make room
        // for the one just returned.
        let mut channels = Vec::new();
        for index in 0..MAX_IDLE_ENVIRONMENTS {
            let (standing, incoming) = environment();
            kept_in(&mut pool, &key(&index.to_string()), standing, 1);
            channels.push(incoming);
        }
        assert_eq!(
            pool.held, MAX_IDLE_ENVIRONMENTS,
            "case the pool at its count: expected {MAX_IDLE_ENVIRONMENTS} held, got {}",
            pool.held
        );
        let (standing, _incoming) = environment();
        kept_in(&mut pool, &key("another"), standing, 1);
        assert_eq!(
            pool.held, MAX_IDLE_ENVIRONMENTS,
            "case one more returned: the count must still hold, got {}",
            pool.held
        );
        assert!(
            channels
                .first()
                .is_some_and(|incoming| incoming.recv().is_err()),
            "case one more returned: the longest-idle environment must have been retired"
        );
        assert!(
            taken_from(&mut pool, &key("another")).is_some(),
            "case one more returned: the environment that made room must be the one held"
        );

        // And past the MEMORY the standing environments may hold between them, the same: an
        // environment that cannot fit even an empty pool is not kept at all.
        let mut pool = Pool::default();
        let (standing, first) = environment();
        kept_in(&mut pool, &key("one"), standing, MAX_STANDING_BYTES);
        let (standing, second) = environment();
        kept_in(&mut pool, &key("two"), standing, MAX_STANDING_BYTES);
        assert_eq!(
            (pool.held, pool.held_bytes),
            (1, MAX_STANDING_BYTES),
            "case two environments of the whole envelope: only one may stand, got {} and {}",
            pool.held,
            pool.held_bytes
        );
        assert!(
            first.recv().is_err(),
            "case two environments of the whole envelope: the longest-idle one must have been \
             retired"
        );
        assert!(
            taken_from(&mut pool, &key("two")).is_some(),
            "case two environments of the whole envelope: the later one must be the one held"
        );
        drop(second);
        let (standing, oversized) = environment();
        kept_in(
            &mut pool,
            &key("three"),
            standing,
            MAX_STANDING_BYTES.saturating_add(1),
        );
        assert_eq!(
            (pool.held, pool.held_bytes),
            (0, 0),
            "case an environment larger than the whole envelope: nothing may be held for it, \
             got {} and {}",
            pool.held,
            pool.held_bytes
        );
        assert!(
            oversized.recv().is_err(),
            "case an environment larger than the whole envelope: its thread must see its \
             channel closed"
        );
    }

    /// An environment is handed back to the invocation of the SAME key and to no other, and
    /// a key that has none standing takes none.
    #[test]
    fn an_environment_is_taken_only_by_an_invocation_of_its_own_key() {
        let mut pool = Pool::default();
        let (standing, incoming) = environment();
        kept_in(&mut pool, &key("one"), standing, 0);
        assert!(
            taken_from(&mut pool, &key("two")).is_none(),
            "case another revision of the same function: no environment may be served to it"
        );
        // Retiring it ends the thread's side of the channel, which is what stops an
        // environment nothing will use again.
        assert!(
            retire_longest_idle(&mut pool),
            "case one environment held: it must be retired"
        );
        assert_eq!(
            pool.held, 0,
            "case retired: nothing is held any more, got {}",
            pool.held
        );
        assert!(
            incoming.recv().is_err(),
            "case retired: the environment's thread must see its channel closed"
        );
        assert!(
            !retire_longest_idle(&mut pool),
            "case nothing held: there is nothing to retire"
        );
    }

    /// Which completions leave an environment one a further invocation can be served in.
    ///
    /// The rows are one per variant a run can come to.
    #[test]
    fn only_a_completed_invocation_leaves_its_environment_standing() {
        let cases: &[(&str, Completion, bool)] = &[
            (
                "a handler that returned",
                Completion::Returned("{}".to_string()),
                true,
            ),
            (
                "a handler that raised",
                Completion::Raised {
                    error_type: "ValueError".to_string(),
                    error_message: "boom".to_string(),
                    stack_trace: Vec::new(),
                    initialization: false,
                },
                true,
            ),
            (
                "a handler that streamed its answer",
                Completion::Streamed {
                    chunks: vec!["part".to_string()],
                    failure: None,
                },
                true,
            ),
            (
                "a run the deadline ended",
                Completion::Stopped(super::super::outcome::Stop::Deadline),
                false,
            ),
            (
                "a run the step budget ended",
                Completion::Stopped(super::super::outcome::Stop::Steps),
                false,
            ),
            (
                "a run the memory ceiling ended",
                Completion::Stopped(super::super::outcome::Stop::Memory),
                false,
            ),
            (
                "a run whose response was too large",
                Completion::Stopped(super::super::outcome::Stop::Response),
                false,
            ),
            (
                "a run that could not be driven",
                Completion::Internal("no engine".to_string()),
                false,
            ),
        ];
        for (case, completion, expected) in cases {
            assert_eq!(
                reusable(completion),
                *expected,
                "case {case}: expected reusable to be {expected}, got {:?}",
                completion
            );
        }
    }

    /// Which RUNTIMES a function's provisioning stands an environment up for: only the ones
    /// whose runs happen in the interpreter this module stands up, because an environment of
    /// that kind is the only one there is to stand up ahead of an invocation.
    ///
    /// The rows are one per kind of `Runtime` a function can carry.
    #[test]
    fn only_a_function_of_the_interpreters_own_runtime_is_provisioned_for() {
        let cases: &[(&str, Option<&str>, bool)] = &[
            ("a python runtime", Some("python3.12"), true),
            ("another release of it", Some("python3.13"), true),
            ("a node runtime", Some("nodejs22.x"), false),
            ("a runtime no engine of this emulator runs", Some("go1.x"), false),
            ("an image package, which names no runtime", None, false),
        ];
        for (case, runtime, expected) in cases {
            let actual = super::super::engine_for(*runtime) == Some(super::super::Engine::Python);
            assert_eq!(
                actual, *expected,
                "case {case}: expected an environment to be stood up for it to be {expected}, \
                 got {actual}"
            );
        }
    }

    /// What the pool of BLANK environments does with the ones it is handed: it stands no more
    /// up than this process keeps, counts one that is on its way as one of them, serves the
    /// freshest to the invocation that needs an environment, and serves none that has stood
    /// past the idle span — each blank it does not keep having its channel closed, which is
    /// what tears that environment down.
    #[test]
    fn the_blank_pool_stands_up_what_it_keeps_and_serves_it_to_any_invocation() {
        let mut blanks = Blanks::default();
        // Up to the count this process keeps, every provisioning asks for one more; past it,
        // none does — and the one already on its way counts.
        for index in 0..MAX_STANDING_BLANKS {
            assert!(
                asked_for(&mut blanks),
                "case {index} standing up: a provisioning must ask for one more"
            );
        }
        assert_eq!(
            blanks.building, MAX_STANDING_BLANKS,
            "case every one on its way: expected {MAX_STANDING_BLANKS} being stood up, got {}",
            blanks.building
        );
        assert!(
            !asked_for(&mut blanks),
            "case as many on their way as this process keeps: no further one may be asked for"
        );
        // Each one that stands is parked, and the claim its build held is given back by the
        // guard the thread holds ([`Building`]), which is what this stands in for.
        let mut channels = Vec::new();
        for index in 0..MAX_STANDING_BLANKS {
            let (standing, incoming) = environment();
            assert!(
                matches!(parked_in(&mut blanks, standing), Parked::Waiting),
                "case {index} stood up: it must be held for the next invocation"
            );
            ended_in(&mut blanks);
            channels.push(incoming);
        }
        assert_eq!(
            (blanks.waiting.len(), blanks.building),
            (MAX_STANDING_BLANKS, 0),
            "case every one stood up: expected {MAX_STANDING_BLANKS} standing and none on \
             their way, got {} and {}",
            blanks.waiting.len(),
            blanks.building
        );
        // One more than this process keeps is not held at all: it is refused, and its channel
        // is closed when the caller tears it down.
        let (standing, refused) = environment();
        assert!(
            matches!(parked_in(&mut blanks, standing), Parked::Refused),
            "case one more than this process keeps: it must not be held"
        );
        drop(refused);
        // An invocation of ANY key adopts a standing blank, because a blank has adopted no
        // function yet.
        let taken = taken_blank_from(&mut blanks);
        assert!(
            taken.is_some(),
            "case an invocation that needs an environment: a standing blank must be served to it"
        );
        assert_eq!(
            (blanks.waiting.len(), blanks.adopted),
            (MAX_STANDING_BLANKS - 1, 1),
            "case one adopted: expected {} standing and one adopted, got {} and {}",
            MAX_STANDING_BLANKS - 1,
            blanks.waiting.len(),
            blanks.adopted
        );
        drop(taken);

        // A blank that has stood past the idle span is not served from: it is retired, and the
        // invocation that looked for one stands up its own.
        let mut blanks = Blanks::default();
        let (standing, incoming) = environment();
        assert!(asked_for(&mut blanks), "case the idle span: one is asked for");
        let _ = parked_in(&mut blanks, standing);
        ended_in(&mut blanks);
        for waiting in blanks.waiting.iter_mut() {
            waiting.since -= IDLE_LIFETIME;
        }
        assert!(
            taken_blank_from(&mut blanks).is_none(),
            "case a blank past its idle span: it must not be served from"
        );
        assert!(
            incoming.recv().is_err(),
            "case a blank past its idle span: its thread must see its channel closed"
        );
        assert_eq!(
            (blanks.waiting.len(), blanks.adopted),
            (0, 0),
            "case a blank past its idle span: nothing is held and nothing was adopted, got {} \
             and {}",
            blanks.waiting.len(),
            blanks.adopted
        );
    }

    /// A blank this process asked for and did not get leaves nothing claimed: the next
    /// provisioning asks for one again.
    #[test]
    fn a_blank_that_did_not_stand_up_leaves_nothing_claimed() {
        let mut blanks = Blanks::default();
        assert!(
            asked_for(&mut blanks),
            "case the first provisioning: one is asked for"
        );
        ended_in(&mut blanks);
        assert_eq!(
            blanks.building, 0,
            "case a build that did not stand: nothing may be left claimed, got {}",
            blanks.building
        );
        assert!(
            asked_for(&mut blanks),
            "case the provisioning after it: one must be asked for again"
        );
    }
}
