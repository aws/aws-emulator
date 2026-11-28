// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the THREAD POOL and the reentrant LOCK of the thread interface a
//! `python3.x` function's run is given: `CreateFunction` then `Invoke` (`POST
//! /2015-03-31/functions/{FunctionName}/invocations`) over the router.
//!
//! The rule under test: a managed runtime's execution environment presents the WHOLE thread
//! interface the standard library drives — a reentrant lock carries the ownership, the
//! recursion count and the save/restore calls `threading.Condition` reads off its lock, so a
//! condition built on one waits and wakes instead of raising that its lock is not held — and a
//! thread pool of that interface RUNS every callable it is handed: `submit` and `map` answer
//! futures carrying the value each callable returned or the exception it raised, however many
//! items are handed to however many workers.
//!
//! Every row drives real HTTP requests through the router, and the rows differ along what the
//! rule generalises over: HOW the work is handed to the pool (`map`, `submit` read back by
//! `result`, `submit` read back by `as_completed`, a submit made from inside another item),
//! how many items are handed to how many workers (fewer, as many, and far more than
//! `max_workers`, which is the recorded shape), what a callable does (computes, returns
//! nothing, raises), what the pool was configured with (an initializer that runs, one that
//! raises, and arguments the interface refuses), and which surface of the lock is read (the
//! four calls themselves, a condition built on the default reentrant lock, and a condition
//! built on a lock that is not reentrant).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::invoke_request_id::{is_host_bound, one_invocation_stream_at_a_time};
use crate::python_frame_source_lines::{create_function, invoke, poll_until_active, router_on};

/// What one case of the probe answered, or `None` when the run was ended by a bound this whole
/// test binary shares rather than by anything the case asked for.
///
/// The invocation itself is part of every row's assertion: a handler that drives a pool is
/// answered `200` with no function error.
async fn answered(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    case: &str,
    event: &Value,
) -> Option<Value> {
    let answer = invoke(
        router,
        name,
        &serde_json::to_vec(event).expect("the event serializes"),
    )
    .await;
    if is_host_bound(answer.status, &answer.text()) {
        return None;
    }
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected the invocation to be answered 200, got {} {}",
        answer.status,
        answer.text()
    );
    let document = answer.json();
    assert_eq!(
        document.get("failed"),
        None,
        "case {case:?}: expected the handler to answer rather than fail, got {document}"
    );
    Some(document)
}

/// The value one member of an answer holds, named in the failure when it is missing.
fn member<'answer>(case: &str, document: &'answer Value, name: &str) -> &'answer Value {
    document.get(name).unwrap_or_else(|| {
        panic!("case {case:?}: expected the answer to carry {name:?}, got {document}")
    })
}

/// The whole rule, driven END TO END as one test: one real `CreateFunction` and one real
/// `Invoke` per table over the router.
///
/// One test and one stream of invocations, the way the sibling runtime suites are built: the
/// memory bound a run is stopped on is read from the WHOLE process and this binary runs its
/// suites in parallel, so fewer interpreters in flight at once is fewer runs stopped for memory
/// belonging to another suite. A table whose run was stopped by one of those shared bounds says
/// nothing about the rule and is left out of the reading — but not every one of them can be, so
/// a test that read none of them fails rather than passing silently.
#[tokio::test]
async fn the_python_runtime_runs_the_work_a_thread_pool_is_handed() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let probe = "probe-thread-pool-executor";
    create_function(&router, probe, &[("index.py", PROBE_SOURCE)], "index.handler").await;
    poll_until_active(&router, probe).await;
    let mut read = 0;
    read += usize::from(every_item_a_pool_is_handed_runs(&router, probe).await);
    read += usize::from(a_pool_reports_what_it_was_configured_with(&router, probe).await);
    read += usize::from(the_reentrant_lock_carries_the_calls_a_condition_reads(&router, probe).await);
    assert!(
        read > 0,
        "expected at least one of the tables above to be answered by a run, got {read}: every \
         invocation reported a bound this whole test binary shares, so nothing of the rule was \
         read"
    );
}

/// The recorded shape and its generalisation: every callable handed to a pool RUNS, and its
/// future carries what that call came to.
///
/// Answers whether a run answered this table.
async fn every_item_a_pool_is_handed_runs(
    router: &smithy_server_runtime::MultiServiceRouter,
    probe: &str,
) -> bool {
    let case = "the work items a pool is handed";
    let Some(document) = answered(router, probe, case, &json!({"case": "work"})).await else {
        return false;
    };
    // (the row, the member it answered under, what the callables compute for it)
    let rows: &[(&str, &str, Value)] = &[
        // The recorded shape: far more items than the pool has workers.
        (
            "map over more items than the pool has workers",
            "mapped_over_many",
            json!([0, 1, 4, 9, 16, 25, 36, 49]),
        ),
        (
            "map over as many items as the pool has workers",
            "mapped_over_as_many",
            json!([0, 1]),
        ),
        (
            "map over fewer items than the pool has workers",
            "mapped_over_fewer",
            json!([0]),
        ),
        ("map over no items at all", "mapped_over_none", json!([])),
        (
            "map on a pool configured with one worker",
            "mapped_on_one_worker",
            json!([0, 1, 4, 9, 16]),
        ),
        (
            "map on a pool configured with the default number of workers",
            "mapped_on_default_workers",
            json!([0, 1, 4, 9]),
        ),
        ("a submit read back by result", "submitted", json!(81)),
        (
            "submits read back as they complete",
            "completed",
            json!([0, 1, 4, 9, 16]),
        ),
        (
            "a submit made from inside another item",
            "nested",
            json!(49),
        ),
        (
            "a callable that returns nothing",
            "returned_nothing",
            Value::Null,
        ),
    ];
    for (row, name, computed) in rows {
        assert_eq!(
            member(case, &document, name),
            computed,
            "case {case:?} / {row}: expected the callables' own answers, got {document}"
        );
    }
    // A callable that RAISED is read back off its own future, and the code that submitted it
    // went on: the failure is the work item's and not the pool's.
    assert_eq!(
        member(case, &document, "raised").as_str(),
        Some("ValueError: this work item's own failure"),
        "case {case:?}: expected the item's own exception on its future, got {document}"
    );
    assert_eq!(
        member(case, &document, "after_raising").as_str(),
        Some("the handler went on"),
        "case {case:?}: expected the handler to go on after an item raised, got {document}"
    );
    // Every item ran on a WORKER thread of the pool: the thread a callable reads back is one
    // of the pool's own and not the one the handler runs in.
    assert_eq!(
        member(case, &document, "off_the_handlers_thread"),
        &json!(true),
        "case {case:?}: expected every item to have run on a worker thread of the pool, got \
         {document}"
    );
    assert!(
        member(case, &document, "worker_names")
            .as_array()
            .is_some_and(|names| names.len() > 1
                && names
                    .iter()
                    .all(|name| name.as_str().is_some_and(|name| name.contains("ThreadPool")))),
        "case {case:?}: expected each item to name the worker thread of the pool it ran in, got \
         {document}"
    );
    true
}

/// RULE: what the pool was CONFIGURED with decides what it accepts: an initializer runs before
/// the first item, an initializer that raises breaks the pool, a submit after a shutdown is
/// refused, and arguments the interface does not admit are refused by the constructor.
///
/// Answers whether a run answered this table.
async fn a_pool_reports_what_it_was_configured_with(
    router: &smithy_server_runtime::MultiServiceRouter,
    probe: &str,
) -> bool {
    let case = "what a pool was configured with";
    let Some(document) = answered(router, probe, case, &json!({"case": "configured"})).await else {
        return false;
    };
    // The initializer ran, once, before the first item of the pool it belongs to.
    assert_eq!(
        member(case, &document, "initialized").as_i64(),
        Some(1),
        "case {case:?}: expected the pool's initializer to have run once, got {document}"
    );
    assert_eq!(
        member(case, &document, "initialized_item").as_str(),
        Some("ready"),
        "case {case:?}: expected the item to have run after the initializer, got {document}"
    );
    // (the row, the member, the refusal the interface answers)
    let rows: &[(&str, &str, &str)] = &[
        (
            "a pool whose initializer raised",
            "broken",
            "BrokenThreadPool: A thread initializer failed, the thread pool is not usable \
             anymore",
        ),
        (
            "a submit made after the pool was shut down",
            "after_shutdown",
            "RuntimeError: cannot schedule new futures after shutdown",
        ),
        (
            "a pool asked for no workers at all",
            "no_workers",
            "ValueError: max_workers must be greater than 0",
        ),
        (
            "a pool given an initializer that is not callable",
            "uncallable_initializer",
            "TypeError: initializer must be a callable",
        ),
    ];
    for (row, name, refusal) in rows {
        assert_eq!(
            member(case, &document, name).as_str(),
            Some(*refusal),
            "case {case:?} / {row}: expected the interface's own refusal, got {document}"
        );
    }
    // The number of workers a pool was configured with is the number it reports, and a pool
    // configured with none reports the one the interface computes rather than nothing.
    assert_eq!(
        member(case, &document, "workers_configured").as_i64(),
        Some(3),
        "case {case:?}: expected the pool to report the workers it was configured with, got \
         {document}"
    );
    assert!(
        member(case, &document, "workers_default")
            .as_i64()
            .is_some_and(|workers| workers > 0),
        "case {case:?}: expected a pool configured with no number of workers to report one of \
         its own, got {document}"
    );
    true
}

/// RULE: the reentrant lock carries the calls the standard library drives one through, so a
/// condition built on one — which is what a condition built with no lock of its own is — waits
/// and wakes rather than raising that its lock is not held.
///
/// Answers whether a run answered this table.
async fn the_reentrant_lock_carries_the_calls_a_condition_reads(
    router: &smithy_server_runtime::MultiServiceRouter,
    probe: &str,
) -> bool {
    let case = "the reentrant lock of the thread interface";
    let Some(document) = answered(router, probe, case, &json!({"case": "lock"})).await else {
        return false;
    };
    // The four calls `threading.Condition` reads off the lock it was built on. Source: the
    // reentrant lock of CPython's own `_thread`, which `threading.Condition.__init__` takes
    // `_release_save`, `_acquire_restore` and `_is_owned` off, and `_recursion_count` beside
    // them.
    let calls = member(case, &document, "calls");
    for call in [
        "_is_owned",
        "_recursion_count",
        "_release_save",
        "_acquire_restore",
    ] {
        assert!(
            calls
                .as_array()
                .is_some_and(|held| held.iter().any(|name| name.as_str() == Some(call))),
            "case {case:?}: expected the reentrant lock to carry {call:?}, got {document}"
        );
    }
    // (the row, the member, what the lock's own state says)
    let rows: &[(&str, &str, Value)] = &[
        ("a lock nobody holds is owned by nobody", "owned_free", json!(false)),
        ("a lock the running thread took is owned by it", "owned_held", json!(true)),
        ("the recursion count of a free lock", "count_free", json!(0)),
        ("the recursion count of a lock taken twice", "count_twice", json!(2)),
        (
            "every level given up while waiting is taken back",
            "count_restored",
            json!(2),
        ),
        ("a condition on the default lock waits", "waited", json!(true)),
        ("a condition on the default lock wakes", "notified", json!(true)),
        (
            "a condition on a lock that is not reentrant",
            "plain_notified",
            json!(true),
        ),
        (
            "an event, which the library builds on such a condition",
            "event_set",
            json!(true),
        ),
        (
            "a queue, which the library builds two of them on",
            "queued",
            json!("one"),
        ),
    ];
    for (row, name, held) in rows {
        assert_eq!(
            member(case, &document, name),
            held,
            "case {case:?} / {row}: expected the lock's own state, got {document}"
        );
    }
    true
}

/// The probe every row drives: its handler answers what the case the event names came to.
const PROBE_SOURCE: &str = r#"
import _thread
import queue
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed
from concurrent.futures.thread import BrokenThreadPool


def _square(value):
    return value * value


# WHERE each item of a pool ran: the thread it was running in, and whether that was the thread
# the handler itself runs in.
_WORKERS = []


def _where(value):
    """A work item that reports the worker thread it ran in beside its own answer."""
    running = threading.current_thread()
    _WORKERS.append((running.name, running is threading.main_thread()))
    return value * value


def _raises(value):
    raise ValueError("this work item's own failure")


def _nothing(value):
    return None


def _work(event):
    """Work handed to a pool every way the interface offers."""
    answered = {}
    with ThreadPoolExecutor(max_workers=2) as pool:
        answered["mapped_over_many"] = list(pool.map(_square, range(8)))
        answered["mapped_over_as_many"] = list(pool.map(_square, range(2)))
        answered["mapped_over_fewer"] = list(pool.map(_square, range(1)))
        answered["mapped_over_none"] = list(pool.map(_square, []))
        answered["submitted"] = pool.submit(_square, 9).result()
        futures = [pool.submit(_square, index) for index in range(5)]
        answered["completed"] = sorted(future.result() for future in as_completed(futures))
        answered["returned_nothing"] = pool.submit(_nothing, 1).result()
        failing = pool.submit(_raises, 3)
        raised = failing.exception()
        answered["raised"] = type(raised).__name__ + ": " + str(raised)
        answered["after_raising"] = "the handler went on"

        def _nests(value):
            return pool.submit(_square, value).result()

        answered["nested"] = pool.submit(_nests, 7).result()
    with ThreadPoolExecutor(max_workers=2) as watched:
        list(watched.map(_where, range(4)))
        answered["worker_names"] = [name for name, _ in _WORKERS]
        answered["off_the_handlers_thread"] = all(not main for _, main in _WORKERS)
    with ThreadPoolExecutor(max_workers=1) as one:
        answered["mapped_on_one_worker"] = list(one.map(_square, range(5)))
    with ThreadPoolExecutor() as default:
        answered["mapped_on_default_workers"] = list(default.map(_square, range(4)))
    return answered


def _configured(event):
    """What a pool's own configuration decides."""
    answered = {}
    ran = []

    def _initializer():
        ran.append(1)

    with ThreadPoolExecutor(max_workers=3, initializer=_initializer) as pool:
        answered["initialized_item"] = pool.submit(lambda: "ready").result()
        pool.submit(lambda: "again").result()
        answered["initialized"] = len(ran)
        answered["workers_configured"] = pool._max_workers
    with ThreadPoolExecutor() as default:
        answered["workers_default"] = default._max_workers

    def _breaks():
        raise RuntimeError("this initializer's own failure")

    broken = ThreadPoolExecutor(max_workers=1, initializer=_breaks)
    failed = broken.submit(lambda: "never run").exception()
    answered["broken"] = type(failed).__name__ + ": " + str(failed)
    try:
        broken.submit(lambda: "never run")
        answered["broken_again"] = "no error"
    except BrokenThreadPool as error:
        answered["broken_again"] = type(error).__name__ + ": " + str(error)
    broken.shutdown()
    pool = ThreadPoolExecutor(max_workers=1)
    pool.shutdown()
    try:
        pool.submit(_square, 1)
        answered["after_shutdown"] = "no error"
    except BaseException as error:
        answered["after_shutdown"] = type(error).__name__ + ": " + str(error)
    try:
        ThreadPoolExecutor(max_workers=0)
        answered["no_workers"] = "no error"
    except BaseException as error:
        answered["no_workers"] = type(error).__name__ + ": " + str(error)
    try:
        ThreadPoolExecutor(max_workers=1, initializer="not a callable")
        answered["uncallable_initializer"] = "no error"
    except BaseException as error:
        answered["uncallable_initializer"] = type(error).__name__ + ": " + str(error)
    return answered


def _lock(event):
    """The reentrant lock's own surface, and the conditions the library builds on it."""
    answered = {}
    lock = _thread.RLock()
    answered["calls"] = [
        name
        for name in ("_is_owned", "_recursion_count", "_release_save", "_acquire_restore")
        if hasattr(lock, name)
    ]
    answered["owned_free"] = lock._is_owned()
    answered["count_free"] = lock._recursion_count()
    lock.acquire()
    answered["owned_held"] = lock._is_owned()
    lock.acquire()
    answered["count_twice"] = lock._recursion_count()
    given_up = lock._release_save()
    lock._acquire_restore(given_up)
    answered["count_restored"] = lock._recursion_count()
    lock.release()
    lock.release()
    # A condition built with no lock of its own is built on a reentrant one, which is what
    # `concurrent.futures.Future` and every handler writing `threading.Condition()` stand on.
    reentrant = threading.Condition()
    with reentrant:
        answered["waited"] = reentrant.wait(0.01) is not None
        reentrant.notify_all()
        answered["notified"] = True
    plain = threading.Condition(threading.Lock())
    with plain:
        plain.notify_all()
        answered["plain_notified"] = True
    flag = threading.Event()
    flag.set()
    answered["event_set"] = flag.is_set() and flag.wait(0.01)
    holding = queue.Queue()
    holding.put("one")
    answered["queued"] = holding.get()
    return answered


_CASES = {"work": _work, "configured": _configured, "lock": _lock}


def handler(event, context):
    try:
        return _CASES[event["case"]](event)
    except BaseException as error:
        return {"failed": type(error).__name__ + ": " + str(error)}
"#;
