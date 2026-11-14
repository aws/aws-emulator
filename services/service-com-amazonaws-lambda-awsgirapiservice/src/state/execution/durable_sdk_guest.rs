// SPDX-License-Identifier: CC0-1.0

//! The DURABLE-EXECUTION programming model a Python handler is written against: the
//! `aws_durable_execution_sdk_python` module this runtime supplies, in Python.
//!
//! AWS Lambda's managed `python3.x` runtimes ship the durable-execution SDK on a function's
//! import path, the way they ship the AWS SDK ([`super::aws_sdk_guest`]), so a handler
//! written against the durable programming model — decorated `@durable_execution`, waiting
//! with `context.wait(Duration.from_seconds(…))`, running a unit of work through
//! `context.step(…)` — imports it without shipping a dependency of its own. This module is
//! that import path's contents here.
//!
//! What it does:
//!
//! * `@durable_execution` is what makes a handler a durable function's entry point: Lambda
//!   calls `handler(event, context)`, and the decorator hands the handler the DURABLE
//!   context — the invocation's own context plus the durable operations below — so the
//!   handler's `context.wait(…)` and `context.step(…)` resolve. Every other member a
//!   handler reads off its context (`aws_request_id`, `get_remaining_time_in_millis()`, the
//!   function's name and version) is the invocation's own, read straight through.
//! * `context.wait(duration)` waits for the span the handler asked for, so the execution is
//!   still `RUNNING` for that long and the handler continues afterwards. The duration is the
//!   handler's own parameter — nothing here shortens, lengthens or skips it — and like every
//!   other way a handler spends time it is spent inside the invocation: the wait is the
//!   standard library's own sleep, which this runtime hands the guest already clamped to what
//!   the run has left ([`super::python`]), so a wait the function's own `Timeout` cuts off
//!   ends the run as any other unfinished handler's does.
//! * `context.step(work, …)` RUNS `work` and answers what it returned, and `@durable_step`
//!   marks a callable as one such unit. A durable execution of this emulator runs once, so
//!   the first run is the only run: there is no replay for a completed step to be skipped
//!   on, and nothing here reports a step result that `work` did not produce. For that same
//!   reason neither a step's nor a wait's own NAME is read, and neither is a retry strategy
//!   a caller configures for a step: a unit that raises fails the execution, which is what
//!   the execution's error reports, rather than being attempted again.
//! * `Duration` is the span type the model's own operations take, with the constructors a
//!   handler writes (`from_millis`, `from_seconds`, `from_minutes`, `from_hours`,
//!   `from_days`) and the readings it gives back.
//!
//! What it does NOT do: the CALLBACK operations of the model
//! (`context.wait_for_callback`), which mint a callback id a client then signals. A handler
//! reaching for one is answered the attribute error of a name this runtime does not supply,
//! rather than a callback id nothing can be signalled through.

use super::package::PackageModule;

/// The modules this runtime puts on a Python handler's import path as the durable-execution
/// SDK it ships.
///
/// Resolved like the AWS SDK's ([`super::aws_sdk_guest::runtime_modules`]): after the
/// deployment package's own entries, so a package shipping its own copy wins.
#[must_use]
pub fn runtime_modules() -> Vec<PackageModule> {
    [(
        "aws_durable_execution_sdk_python/__init__.py",
        DURABLE_SDK_SOURCE,
    )]
    .into_iter()
    .map(|(path, source)| PackageModule {
        path: path.to_string(),
        source: source.to_string(),
    })
    .collect()
}

/// `aws_durable_execution_sdk_python`: the names a durable handler imports.
const DURABLE_SDK_SOURCE: &str = r#""""The durable-execution programming model for Python handlers."""

import time

__all__ = [
    "Duration",
    "DurableContext",
    "durable_execution",
    "durable_step",
]

class Duration:
    """A span of time, as this model's own operations take one."""

    def __init__(self, seconds):
        self._seconds = float(seconds)

    @classmethod
    def from_millis(cls, millis):
        return cls(float(millis) / 1000.0)

    @classmethod
    def from_seconds(cls, seconds):
        return cls(seconds)

    @classmethod
    def from_minutes(cls, minutes):
        return cls(float(minutes) * 60.0)

    @classmethod
    def from_hours(cls, hours):
        return cls(float(hours) * 3600.0)

    @classmethod
    def from_days(cls, days):
        return cls(float(days) * 86400.0)

    def total_seconds(self):
        return self._seconds

    def total_millis(self):
        return int(self._seconds * 1000.0)

    def __eq__(self, other):
        if isinstance(other, Duration):
            return self._seconds == other._seconds
        return NotImplemented

    def __hash__(self):
        return hash(self._seconds)

    def __repr__(self):
        return "Duration(seconds=%r)" % (self._seconds,)


def _seconds_of(duration):
    """The span `duration` names, in seconds."""
    if isinstance(duration, Duration):
        return duration.total_seconds()
    if isinstance(duration, bool):
        raise TypeError("a duration is a Duration or a number of seconds, not a bool")
    if isinstance(duration, (int, float)):
        return float(duration)
    reading = getattr(duration, "total_seconds", None)
    if reading is not None:
        return float(reading())
    millis = getattr(duration, "total_millis", None)
    if millis is not None:
        return float(millis()) / 1000.0
    raise TypeError(
        "a duration is a Duration or a number of seconds, not %r" % (type(duration),)
    )


def _named_like(wrapper, target):
    """Carry `target`'s own name and documentation onto the wrapper around it."""
    for member in ("__name__", "__qualname__", "__doc__", "__module__"):
        try:
            setattr(wrapper, member, getattr(target, member))
        except AttributeError:
            pass
    wrapper.__wrapped__ = target
    return wrapper


class DurableContext:
    """One invocation's context, with this model's durable operations on it.

    Every member this class does not define is the invocation context's own.
    """

    def __init__(self, context):
        self._context = context

    def __getattr__(self, name):
        return getattr(self._context, name)

    def __repr__(self):
        return "DurableContext(%r)" % (self._context,)

    @property
    def context(self):
        """The invocation context this durable context was built from."""
        return self._context

    def wait(self, duration, name=None, **options):
        """Wait for `duration` before the handler continues.

        The wait's own NAME, and any further option a caller passes for it, are not read,
        for the reason `step` records: this execution runs once, so there is no replay for a
        named checkpoint to be matched against.
        """
        seconds = _seconds_of(duration)
        if seconds > 0:
            time.sleep(seconds)

    def step(self, work, *args, **kwargs):
        """Run `work` as one unit of this execution and answer what it returned.

        The unit's own NAME, and the configuration a caller may pass for it, are not read:
        this execution runs once, so there is no replay for a named checkpoint to be matched
        against and no retry state for a strategy to drive. A unit that raises therefore
        fails the execution here rather than being retried, which is what the execution's
        error reports.
        """
        kwargs.pop("name", None)
        kwargs.pop("config", None)
        return work(*args, **kwargs)


def durable_execution(handler=None, **options):
    """Mark a handler as a durable function's entry point.

    Usable bare (`@durable_execution`) and called (`@durable_execution(...)`).
    """

    def decorate(target):
        def invoke(event, context):
            return target(event, DurableContext(context))

        invoke.durable_execution = True
        return _named_like(invoke, target)

    if handler is None:
        return decorate
    return decorate(handler)


def durable_step(work=None, name=None, **options):
    """Mark a callable as one unit of work of a durable execution.

    Usable bare (`@durable_step`) and called (`@durable_step(name="...")`). Calling the
    marked callable runs it, here and in `DurableContext.step`.
    """

    def decorate(target):
        def run(*args, **kwargs):
            return target(*args, **kwargs)

        run.durable_step = name or getattr(target, "__name__", "step")
        return _named_like(run, target)

    if work is None:
        return decorate
    return decorate(work)
"#;
