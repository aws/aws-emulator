// SPDX-License-Identifier: CC0-1.0

//! The THREAD POOL of this runtime's standard library: the `concurrent.futures`
//! `ThreadPoolExecutor` a Python handler hands its work to.
//!
//! A `python3.x` runtime is a promise about an interpreter AND about the standard library that
//! comes with it, and a pool of worker threads is how a handler fans its own work out: it
//! submits callables, reads the futures back and expects every callable to have RUN. So they
//! run here — each callable is called with its own arguments, and the future carries the value
//! it returned or the exception it raised. Nothing about a work item is synthesised: a result,
//! a failure or a cancellation here is the one the handler's own code produced.
//!
//! What this module replaces, and why it is this module's own rather than the frozen one: the
//! standard library's pool keeps worker threads of the PROCESS alive, parked on its work queue
//! between items. An invocation of this emulator is one interpreter on one thread and every
//! bound it is judged against belongs to that thread ([`super::python`]), so the threads it
//! starts are COOPERATIVE — a body runs on the invocation's own thread, inside the call that
//! started it ([`super::guest_thread`]) — and a worker parked on a queue no other thread can
//! fill is a worker that cannot come back for the next item. The frozen pool, driven that way,
//! performs the first items it is handed and leaves the rest standing: a `map` over more items
//! than it starts workers for never answers for the ones nobody took.
//!
//! This pool is the same pool WITHOUT that parking: a work item is performed by a worker THREAD
//! the pool starts for it through that same cooperative interface, so the item runs as a thread
//! of its own — which is what a callable handed to a pool reads back about where it is running
//! — on the invocation's own thread. Its surface is the standard library's: the `submit` call,
//! the `map` and the `shutdown` its base class drives through it, the future each submit
//! answers, the bounds its arguments are checked against and the refusals of a pool that is
//! shut down or whose initializer failed.
//!
//! The consequences are stated rather than hidden:
//!
//! * two work items never run at the same instant, so a pool makes no handler faster — what it
//!   makes is a handler that uses a pool RUN;
//! * an item is performed where it was submitted, before the code that submitted it goes on, so
//!   every item is already performed by the time its submit answers: a shutdown has nothing left
//!   to wait for and a cancellation nothing left to cancel, which is what the future of a
//!   performed item says for itself;
//! * `max_workers` therefore decides nothing about how the items are performed; it is checked as
//!   the standard library checks it and reported as the pool was configured with it;
//! * a worker's body runs inside a SLICE of the invocation's own budget, exactly as every other
//!   thread this environment starts does ([`super::guest_thread`]), so an item that cannot
//!   finish leaves the invocation the rest of its budget;
//! * past what the thread interface may start for ONE invocation, an item is performed on the
//!   invocation's own thread directly — the thread every worker of this pool really runs on — so
//!   a pool handed more items than that RUNS them rather than refusing them.

use super::package::PackageModule;

/// The modules of the standard library this half of the runtime supplies: the thread pool,
/// under the name the standard library's own pool is published from
/// (`concurrent.futures.thread`), so `concurrent.futures.ThreadPoolExecutor` IS this pool.
///
/// Resolved before the frozen standard library and after the deployment package's own entries,
/// beside the socket, TLS, database and time-zone interfaces this runtime supplies — so the
/// rest of `concurrent.futures` (the executor base, the future, the waiting calls) stays the
/// frozen library's own and a package shipping its own pool still wins.
#[must_use]
pub fn runtime_stdlib_modules() -> Vec<PackageModule> {
    vec![PackageModule {
        path: "concurrent/futures/thread.py".to_string(),
        source: POOL_SOURCE.to_string(),
    }]
}

/// `concurrent.futures.thread`: the pool whose work items are performed on the invocation's own
/// thread.
const POOL_SOURCE: &str = r#"
"""The thread pool of this runtime: the callables it is handed are performed in place."""

import itertools
import os
import threading
import weakref

from concurrent.futures import _base

__all__ = ("BrokenThreadPool", "ThreadPoolExecutor")

# What a pool whose initializer failed answers every further submit with, and what the work
# item that was being performed when it failed is failed with.
_INITIALIZER_FAILED = "A thread initializer failed, the thread pool is not usable anymore"

# The pools this execution environment has built, which is what the name of a pool's worker
# threads counts from, exactly as the standard library's own module counts them.
_counter = itertools.count().__next__

# The work queue of each worker thread a pool holds, as the standard library's own module keeps
# them. A pool of this runtime parks no worker on a queue, so nothing is ever put in it; it is
# here because a program that reaches for it finds the mapping it expects.
_threads_queues = weakref.WeakKeyDictionary()


class BrokenThreadPool(_base.BrokenExecutor):
    """Raised when a worker thread of a `ThreadPoolExecutor` failed to initialize."""


def _processors():
    """How many processors this execution environment reports, and `None` when it reports
    none: what the number of workers a pool was not configured with is computed from."""
    for name in ("process_cpu_count", "cpu_count"):
        counting = getattr(os, name, None)
        if counting is None:
            continue
        try:
            return counting()
        except Exception:
            return None
    return None


class ThreadPoolExecutor(_base.Executor):
    """An executor whose work items are performed by worker threads it starts for them.

    `submit` has the callable it is handed called, with the arguments it is handed, on a worker
    thread of this pool, and answers a future carrying what that call came to; `map`, `shutdown`
    and the context-manager protocol are the base executor's own, driven through this `submit`.
    """

    def __init__(
        self, max_workers=None, thread_name_prefix="", initializer=None, initargs=()
    ):
        if max_workers is None:
            # The default the standard library computes from the machine the pool is on: the
            # processors available to it, and a few more for the items that wait rather than
            # compute. An environment that reports no processor count of its own is counted as
            # the one the library counts it as.
            max_workers = min(32, (_processors() or 1) + 4)
        elif max_workers <= 0:
            raise ValueError("max_workers must be greater than 0")
        if initializer is not None and not callable(initializer):
            raise TypeError("initializer must be a callable")
        self._max_workers = max_workers
        self._initializer = initializer
        self._initargs = initargs
        self._initialized = initializer is None
        self._broken = False
        self._shutdown = False
        self._shutdown_lock = threading.Lock()
        # The worker threads this pool has started, and how many it has started: a worker is
        # named after the pool and the one before it, exactly as the standard library names
        # them.
        self._threads = set()
        self._started = 0
        self._thread_name_prefix = thread_name_prefix or (
            "ThreadPoolExecutor-%d" % _counter()
        )

    def submit(self, fn, /, *args, **kwargs):
        """Perform `fn(*args, **kwargs)` and answer the future it came to."""
        with self._shutdown_lock:
            if self._broken:
                raise BrokenThreadPool(self._broken)
            if self._shutdown:
                raise RuntimeError("cannot schedule new futures after shutdown")
            future = _base.Future()
        self._perform(future, fn, args, kwargs)
        return future

    def _perform(self, future, fn, args, kwargs):
        """Perform ONE work item on a worker thread of this pool.

        The worker is a thread of this execution environment: its body runs on the invocation's
        own thread, inside the call that started it, so the item is performed before the code
        that submitted it goes on — and the callable reads back the worker thread it is running
        in rather than the one the handler runs in. Past what the thread interface will start
        for one invocation, the item is performed on the invocation's own thread directly, which
        is the thread every one of these workers really runs on.
        """
        if not self._initialize(future):
            return
        worker = threading.Thread(
            name="%s_%d" % (self._thread_name_prefix, self._started),
            target=self._item,
            args=(future, fn, args, kwargs),
        )
        self._started += 1
        try:
            worker.start()
        except RuntimeError:
            # The thread interface will start no further thread for this invocation, so the
            # item is performed on the invocation's own thread.
            self._item(future, fn, args, kwargs)
            return
        self._threads.add(worker)

    def _item(self, future, fn, args, kwargs):
        """The body of one work item, reporting onto its own future.

        What the callable came to is the FUTURE's and not the submitting code's, exactly as it
        is on a platform whose pool really has worker threads: an item that raised is read back
        off its future, and the code that submitted it goes on.
        """
        if not future.set_running_or_notify_cancel():
            return
        try:
            answered = fn(*args, **kwargs)
        except BaseException as error:
            future.set_exception(error)
        else:
            future.set_result(answered)

    def _initialize(self, future):
        """Run this pool's initializer, once, before its first work item is performed.

        A pool's initializer runs on each worker thread before that worker takes work, and the
        workers of this pool are the invocation's own thread, so it runs once: before the first
        item. One that raises BREAKS the pool, which is what the item it was raised for and
        every later submit are answered with.
        """
        if self._initialized:
            return True
        self._initialized = True
        try:
            self._initializer(*self._initargs)
        except BaseException:
            self._broken = _INITIALIZER_FAILED
            future.set_exception(BrokenThreadPool(self._broken))
            return False
        return True

    def shutdown(self, wait=True, *, cancel_futures=False):
        """Take this pool out of use: no further work may be submitted to it.

        Every item this pool was handed was performed as it was submitted, so there is nothing
        standing to wait for and nothing standing to cancel: `wait` and `cancel_futures` decide
        about work items that are still pending, of which a pool of this runtime never has any,
        and the worker threads it started are over.
        """
        with self._shutdown_lock:
            self._shutdown = True
"#;
