// SPDX-License-Identifier: CC0-1.0

//! The EXECUTION ENVIRONMENTS this process keeps standing, and which invocation each may
//! serve.
//!
//! An invocation of a `python3.x` function runs in an execution environment: an interpreter
//! with this emulator's runtime installed and the function's own handler module imported
//! ([`super::python::Prepared`]). Building one is the INIT phase, and none of it depends on
//! the invocation that is served in it — which is why Lambda does not build one per call:
//! "After a Lambda function is invoked, Lambda maintains the execution environment for some
//! time in anticipation of another invocation", and an invocation an already-initialized
//! environment serves "skips the initialization phase". That reuse is also what makes the
//! global state a handler leaves behind — a client it created outside the handler, a cached
//! value — be there for the next invocation, which AWS documents handlers may rely on.
//!
//! So this module holds the environments that are standing and idle, keyed by WHICH
//! invocations they may serve ([`EnvironmentKey`]): one function's one version, at one
//! revision of its configuration, in one Region, for one tenant, with one kind of
//! initialization. An invocation that finds no environment of its own builds one; every other
//! invocation is served by the environment already standing, and pays none of that cost.
//!
//! An environment is a THREAD, because the interpreter and every object in it belong to the
//! thread they were built on: an invocation reaches one by being handed to its thread, which
//! answers what the run came to. Exactly one invocation is ever in flight in an environment —
//! an environment being used is OUT of this pool for as long as it is — so the invocations of
//! one function version are served in parallel by as many environments as arrive at once, as
//! many of them at an instant as the memory envelope admits runs ([`super::run_invocation`]).
//!
//! What a STANDING environment holds is bounded on its own, because that memory is held
//! outside the per-invocation envelope: an environment is kept only while the interpreter in
//! it has grown no further than the memory its function is configured for
//! ([`python::Prepared::grown_bytes`]) and has served fewer than
//! [`MAX_INVOCATIONS_PER_ENVIRONMENT`] invocations, and the environments standing idle are
//! held together to [`MAX_IDLE_ENVIRONMENTS`] of them and [`MAX_STANDING_BYTES`] of memory
//! between them, the longest-idle retired until both fit, and every one of them past
//! [`IDLE_LIFETIME`] retired with them — on the next operation the pool performs, because
//! nothing of this emulator polls a pool nobody is using.
//!
//! Three things end an environment, and each ends its THREAD, which finalizes its interpreter
//! and gives that memory back. An invocation the HOST stopped leaves it mid-frame, so it is
//! not kept ([`reusable`]) — which is what AWS does with the environment of a run its timeout
//! ended. One of the bounds above being reached retires it. And so does the pool dropping it
//! to make room for another.

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::{channel, Receiver, SendError, Sender};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use super::aws_api::AwsAccess;
use super::outcome::{Budget, Completion, Finished};
use super::package::PackageModule;
use super::python;
use super::run_clock::Clock;

/// How long an environment nothing has used is kept standing.
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
const IDLE_LIFETIME: Duration = Duration::from_secs(60);

/// How many idle environments this process keeps at once.
///
/// One of the two bounds on what the standing environments hold of this host, beside
/// [`MAX_STANDING_BYTES`]: no more of them stand than the number of runs the engine-memory
/// envelope admits at one instant ([`super::MAX_CONCURRENT_RUNS`]), and the longest-idle
/// environment is retired to make room for a newly returned one. So what the pool holds does
/// not grow with the number of function versions a client invokes.
const MAX_IDLE_ENVIRONMENTS: usize = super::MAX_CONCURRENT_RUNS as usize;

/// How much memory the idle environments of this process hold between them, at most.
///
/// The other bound, and the one that matters when the functions are large: an environment
/// that is STANDING holds its interpreter's memory outside the per-invocation envelope (the
/// share a run claims goes back when the invocation is answered), so the pool sums what each
/// of its environments has grown by ([`python::Prepared::grown_bytes`]) and retires the
/// longest-idle ones until the sum fits. The figure is the same envelope the runs in flight
/// hold between them ([`super::MAX_CONCURRENT_RUN_MEMORY_MB`]), so what this process holds for
/// the handlers it has run is bounded by that envelope again rather than by nothing.
const MAX_STANDING_BYTES: u64 = super::MAX_CONCURRENT_RUN_MEMORY_MB as u64 * 1024 * 1024;

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
    /// What this invocation may spend.
    pub budget: Budget,
    /// How its handler reaches the rest of this emulator.
    pub aws: Option<AwsAccess>,
}

impl Served {
    /// This invocation as the run the interpreter is handed.
    fn as_run(&self) -> python::Run<'_> {
        python::Run {
            modules: &self.modules,
            handler: &self.handler,
            environment: &self.environment,
            event: self.event.as_deref(),
            request_id: &self.request_id,
            invoked_function_arn: &self.invoked_function_arn,
            client_context: self.client_context.as_deref(),
            tenant_id: self.tenant_id.as_deref(),
            budget: self.budget,
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
        let standing = match standing_first.then(|| checkout(key)).flatten().or_else(start) {
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
        elapsed: Duration::ZERO,
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

/// Start an environment: a thread that builds one for the first invocation it is handed and
/// then serves invocation after invocation.
fn start() -> Option<Standing> {
    let (jobs, incoming) = channel();
    ::std::thread::Builder::new()
        .name("lambda-execution-environment".to_string())
        .stack_size(super::RUN_STACK_BYTES)
        .spawn(move || serve_jobs(incoming))
        .ok()?;
    Some(Standing { jobs })
}

/// One execution environment's whole life, on its own thread.
fn serve_jobs(jobs: Receiver<Job>) {
    // Nothing is built before there is an invocation to serve: the first one that arrives is
    // what the environment is built for, and it is the invocation that pays the INIT phase.
    let Ok(first) = jobs.recv() else {
        return;
    };
    let clock = Clock::host();
    let initializing = Instant::now();
    // What the environment is finally torn down against: the budget of the LAST invocation it
    // served, so a guest `atexit` callback is bounded like the handlers were.
    let mut budget = first.served.budget;
    let prepared = match python::Prepared::start(&first.served.as_run(), &clock) {
        Ok(prepared) => prepared,
        Err(failed) => {
            // An initialization that failed is answered to the invocation that asked for it,
            // and no environment is kept.
            let mut finished = *failed;
            finished.elapsed = initializing.elapsed();
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
        let mut finished = prepared.invoke(&job.served.as_run(), &clock);
        // The invocation that BUILT the environment waited for the initialization as well as
        // for its own run, so that is what its duration is. Every later invocation waited for
        // its own run alone, which is the whole point of the environment still standing.
        if let Some(initializing) = cold.take() {
            finished.elapsed = initializing.elapsed();
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
    prepared.finalize(budget, &clock);
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
}
