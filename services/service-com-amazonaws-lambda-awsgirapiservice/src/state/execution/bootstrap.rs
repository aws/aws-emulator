// SPDX-License-Identifier: CC0-1.0

//! The RUNTIME the handler runs under, in the language the handler is written in.
//!
//! A Python runtime's job is small and entirely defined by what the handler sees: import
//! the handler's module out of the deployment package, build the context object, call the
//! handler with the event and the context, and report back either what it returned or the
//! error document Lambda answers for an exception. That is this source: it runs INSIDE the
//! interpreter [`super::python`] builds, because every one of those steps is a Python one —
//! `importlib` resolves the module, `json` reads the event and writes the response, and
//! `traceback` renders the stack of an exception the handler raised.
//!
//! What it is NOT is the containment. Nothing here is a security boundary: a module the
//! guest must not reach is not registered by the host at all ([`super::python`]), so this
//! source cannot be made to hand one over by deleting or rebinding a name in it. The two
//! host functions it does use are the ones the host deliberately exposes — the log the
//! handler's output goes into, and the bounded sleep — and both are enforced in host code.
//! No bound of the run is reachable from here either: this source neither starts nor moves
//! the deadline the handler's run is stopped at, which is why the protocol below is TWO
//! calls the host makes rather than one.
//!
//! The protocol between host and runtime is a document in and two functions out, so the
//! host's own interpreter surface stays as small as a string in and a string out:
//!
//! * `__lambda_input` — a JSON document the host hands to the first call below: the
//!   handler name, the package's modules, the modules the RUNTIME supplies beside them
//!   (the AWS SDK), the event and the environment the invocation runs with.
//! * `__lambda_prepare` — the INIT phase: it installs this runtime, imports the handler's
//!   module and resolves its entry point, answering the empty string when the environment
//!   is ready and an error document for an initialization that failed. None of that is the
//!   handler's run, so none of it is bounded by the function's `Timeout`.
//! * `__lambda_bind` — the NEXT invocation's own request, put on the prepared environment:
//!   the event, the request id, the ARN the call addressed, the client context, the tenant
//!   and the environment the invocation runs with. It answers the empty string when the
//!   request is bound and an error document when it could not be. An execution environment
//!   serves invocation after invocation, so what one invocation brings arrives here rather
//!   than in the initialization above — and the logging handler is re-installed with this
//!   invocation's request id, so every record a run leaves names the invocation that wrote
//!   it.
//! * `__lambda_invoke` — the handler's own run, which the host calls next, having begun the
//!   invoke phase itself and passing the instant the `Timeout` is up at. It answers a JSON
//!   document carrying what the handler returned, or the error it raised.
//!
//! Nothing here imports an operating-system module: the environment the handler sees is
//! installed by the HOST into the module `os` reads it from, and the context's members are
//! read out of the document above rather than out of a module this source would otherwise
//! have to reach for.

/// The Python source of the runtime, evaluated once per execution environment.
pub(super) const RUNTIME_SOURCE: &str = r##"
import sys

# Everything this runtime itself needs, imported once, before the handler's own work
# begins and before the bounds that measure that work are in place.
import datetime
import importlib
import importlib.util
import json
import logging
import time
import traceback

_host = __import__("_lambda_host")

# What the INIT phase resolved, for the INVOKE phase the host calls next: the request
# document, this runtime's own library directory and the handler's entry point.
_PREPARED = {}


class _LogStream:
    """One of the handler's output streams, writing into the invocation's log.

    The interpreter is built without the host's own standard streams, so this is the
    only `sys.stdout`/`sys.stderr` a handler has: `print()` and the logging handler this
    runtime installs (`_install_logging`) reach the invocation's log through it, and
    nothing reaches this process's stdio.
    """

    encoding = "utf-8"
    errors = "backslashreplace"
    name = "<log>"
    mode = "w"
    closed = False

    def write(self, text):
        if not isinstance(text, str):
            text = str(text)
        _host.log(text)
        return len(text)

    def writelines(self, lines):
        for line in lines:
            self.write(line)

    def flush(self):
        return None

    def isatty(self):
        return False

    def readable(self):
        return False

    def writable(self):
        return True

    def seekable(self):
        return False

    def fileno(self):
        raise OSError("the invocation log has no file descriptor")

    def close(self):
        return None


sys.stdout = _LogStream()
sys.stderr = _LogStream()
sys.stdin = None


class _LambdaLogFormatter(logging.Formatter):
    """How this runtime renders ONE logging record into the invocation's log.

    The managed runtime's own text rendering of a record: the level in brackets, the
    instant the record was created in UTC to the millisecond, the invocation's request id
    and the message, tab separated. So a handler that logs through `logging` leaves lines
    a client can read the level and the invocation off, exactly as `print()` leaves the
    text it was given.
    """

    def __init__(self, request_id):
        logging.Formatter.__init__(self)
        self._request_id = request_id

    def formatTime(self, record, datefmt=None):
        whole = int(record.created)
        millis = min(999, int((record.created - whole) * 1000.0))
        at = datetime.datetime.utcfromtimestamp(whole)
        return "%04d-%02d-%02dT%02d:%02d:%02d.%03dZ" % (
            at.year,
            at.month,
            at.day,
            at.hour,
            at.minute,
            at.second,
            millis,
        )

    def format(self, record):
        message = record.getMessage()
        if record.exc_info:
            rendered = traceback.format_exception(
                record.exc_info[0], record.exc_info[1], record.exc_info[2]
            )
            message = "%s\n%s" % (message, "".join(rendered).rstrip("\n"))
        return "[%s]\t%s\t%s\t%s" % (
            record.levelname,
            self.formatTime(record),
            self._request_id,
            message,
        )


def _install_logging(request_id):
    """Give the root logger the handler the managed runtime gives it.

    A record emitted through `logging` reaches no output on its own: with no handler on
    the root logger the library falls back to one that takes nothing below a warning and
    renders the bare message, so a handler logging at the level it set its logger to
    would leave nothing in the function's log group. The managed runtime installs a
    handler instead, which is what makes `logging.getLogger().setLevel(logging.INFO)`
    followed by `logger.info(...)` appear in the log — so this runtime installs one too,
    writing into the invocation's own output.

    The LEVEL is left exactly as the library's own default (a warning), because that is
    the level the managed runtime leaves the root logger at: what a record has to clear is
    the level the handler's own code set, and nothing here lowers or raises it.

    Installed before the handler's module is imported, so a record a module emits while it
    is being imported is carried too, and installed as the ONLY handler, so a handler
    module calling `logging.basicConfig()` adds none beside it and no record is written
    twice.
    """

    handler = logging.StreamHandler(sys.stdout)
    handler.setFormatter(_LambdaLogFormatter(request_id))
    root = logging.getLogger()
    for installed in list(root.handlers):
        root.removeHandler(installed)
    root.addHandler(handler)


class _PackageLoader:
    """Imports one module from the deployment package held in memory."""

    def __init__(self, path, sources):
        self._path = path
        self._sources = sources

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        source = self._sources[self._path]
        filename = "/var/task/" + self._path
        module.__dict__["__file__"] = filename
        code = compile(source, filename, "exec")
        exec(code, module.__dict__)

    def get_source(self, name):
        return self._sources[self._path]

    def is_package(self, name):
        return self._path.endswith("/__init__.py")


class _PackageFinder:
    """Resolves a module name against the deployment package's own entries.

    Installed ahead of every other finder, so the handler's own modules win over a
    standard-library module of the same name, exactly as the package directory does on
    the real runtime's `sys.path`.

    `spec_from_loader` is handed in rather than imported here: this runs INSIDE the
    import machinery, so an import of its own would re-enter the machinery.
    """

    def __init__(self, sources, spec_from_loader):
        self._sources = sources
        self._spec_from_loader = spec_from_loader

    def find_spec(self, name, path=None, target=None):
        key = name.replace(".", "/")
        for candidate, is_package in ((key + "/__init__.py", True), (key + ".py", False)):
            if candidate in self._sources:
                loader = _PackageLoader(candidate, self._sources)
                return self._spec_from_loader(name, loader, is_package=is_package)
        return None


class LambdaContext:
    """The context object the handler is called with.

    Every member is read from the ENVIRONMENT of the invocation, which is where the
    runtime interface reads them from: the function's name, version, memory size and log
    destination are environment variables of the invocation. The host hands that
    environment in, so this runtime needs no operating-system module of its own.
    """

    def __init__(self, request, environment):
        self.aws_request_id = request["requestId"]
        self.invoked_function_arn = request["invokedFunctionArn"]
        self.log_group_name = environment.get("AWS_LAMBDA_LOG_GROUP_NAME")
        self.log_stream_name = environment.get("AWS_LAMBDA_LOG_STREAM_NAME")
        self.function_name = environment.get("AWS_LAMBDA_FUNCTION_NAME")
        self.function_version = environment.get("AWS_LAMBDA_FUNCTION_VERSION")
        self.memory_limit_in_mb = environment.get("AWS_LAMBDA_FUNCTION_MEMORY_SIZE")
        self.identity = None
        self.client_context = request["clientContext"]
        self.tenant_id = request["tenantId"]
        # When this invocation's own run is up, which the host answers when the invoke
        # phase begins: the execution environment's initialization is not part of it.
        self._deadline_epoch_ms = request["deadlineEpochMs"]

    def get_remaining_time_in_millis(self):
        return max(0, int(self._deadline_epoch_ms - time.time() * 1000.0))

    def log(self, message):
        sys.stdout.write(message)


def _report(error_type, error_message, stack_trace):
    """Write the error report this runtime writes for an unhandled exception.

    The runtime reports an exception on the invocation's stderr as well as answering the
    error document, so the invocation's LOG carries what went wrong: the error's type and
    message, then the handler's own traceback. That is what a client reads out of the
    function's log group for a failed invocation.
    """
    sys.stderr.write("[ERROR] %s: %s\n" % (error_type, error_message))
    sys.stderr.write("Traceback (most recent call last):\n")
    for frame in stack_trace:
        sys.stderr.write(frame if frame.endswith("\n") else frame + "\n")


def _raised(error_type, error_message, stack_trace):
    _report(error_type, error_message, stack_trace)
    return {
        "outcome": "raised",
        "errorType": error_type,
        "errorMessage": error_message,
        "stackTrace": stack_trace,
    }


def _runtime_library(environment):
    """The directory the modules of the RUNTIME ITSELF are reported under.

    A managed runtime keeps the standard library it ships under the library directory of
    its own language installation, named for the runtime the function is configured with
    — which the invocation's environment states, as ``AWS_EXECUTION_ENV`` is
    ``AWS_Lambda_<runtime>``.
    """

    marker = "AWS_Lambda_"
    execution_env = environment.get("AWS_EXECUTION_ENV") or ""
    if not execution_env.startswith(marker):
        return ""
    return "/var/lang/lib/" + execution_env[len(marker):]


def _frame_file(filename, library):
    """The FILE a traceback frame names.

    A frame of the deployment package's own module already names its path under the task
    root, and a frame the interpreter renders as a pseudo-file (``<string>``) has no path
    to name. Everything else is a module of the runtime's own standard library, whose
    frame names that module's source file under the runtime's library directory rather
    than the bare module name it was compiled under.
    """

    if not library or filename.startswith("/") or filename.startswith("<"):
        return filename
    path = filename[:-3] if filename.endswith(".py") else filename
    return "%s/%s.py" % (library, path.replace(".", "/"))


def _frames(trace, library):
    """One rendered frame per entry of ``trace``, as the error document carries them.

    A frame is its location line and the source line at that location, and nothing else:
    the fine-grained error-location anchors (the ``~`` and ``^`` run under the source
    line) this interpreter can render are not part of the frame the runtime reports.
    """

    rendered = []
    for frame in traceback.extract_tb(trace):
        text = '  File "%s", line %s, in %s\n' % (
            _frame_file(frame.filename, library),
            frame.lineno,
            frame.name,
        )
        if frame.line:
            text += "    %s\n" % frame.line.strip()
        rendered.append(text)
    return rendered


def _bounded_sleep(seconds):
    return _host.sleep(float(seconds))


def _prepare(request, library):
    """The INIT phase: everything that has to happen before the handler can be called.

    Answers ``None`` once the handler's module is imported and its entry point resolved,
    and an error document for an initialization that failed. What it resolved is kept for
    [_invoke], which the HOST calls next: the phase boundary is the host's to decide, so
    nothing in here — and nothing the deployment package can call — says when the
    handler's own `Timeout` starts running.
    """
    sources = request["modules"]
    sys.meta_path.insert(0, _PackageFinder(sources, importlib.util.spec_from_loader))
    # The modules of the STANDARD LIBRARY this runtime supplies itself — the socket
    # interface a Python program expects of its operating system, and the HTTP client built
    # on one — are resolved ahead of the frozen standard library and behind the deployment
    # package, which is where a runtime's own standard library sits: these ARE this
    # runtime's `socket` and `http.client`, and a package shipping its own still wins.
    runtime_stdlib = request.get("runtimeStdlib") or {}
    if runtime_stdlib:
        sys.meta_path.insert(
            1, _PackageFinder(runtime_stdlib, importlib.util.spec_from_loader)
        )
    # The modules the RUNTIME supplies — the AWS SDK a managed runtime ships — are
    # resolved AFTER the deployment package's own entries and after the standard library,
    # so a package shipping its own copy of one of them wins, exactly as the package
    # directory precedes the runtime's site-packages on a real runtime's `sys.path`.
    runtime_sources = request.get("runtimeModules") or {}
    if runtime_sources:
        sys.meta_path.append(
            _PackageFinder(runtime_sources, importlib.util.spec_from_loader)
        )
    time.sleep = _bounded_sleep
    # Before the handler's module is imported, which is where a module-level record is
    # emitted from.
    _install_logging(request["requestId"])

    handler = request["handler"]
    module_name, separator, attribute = handler.rpartition(".")
    if not separator or not module_name or not attribute:
        return _raised(
            "Runtime.MalformedHandlerName",
            "Bad handler '%s': not enough values to unpack (expected 2, got 1)" % handler,
            [],
        )

    try:
        module = importlib.import_module(module_name)
    except BaseException as error:
        return _raised(
            "Runtime.ImportModuleError",
            "Unable to import module '%s': %s" % (module_name, error),
            [],
        )

    try:
        entry = getattr(module, attribute)
    except AttributeError:
        return _raised(
            "Runtime.HandlerNotFound",
            "Handler '%s' missing on module '%s'" % (attribute, module_name),
            [],
        )

    _PREPARED["request"] = request
    _PREPARED["library"] = library
    _PREPARED["entry"] = entry
    return None


def _invoke(deadline_epoch_ms):
    """The INVOKE phase: the handler's own run, which is what the ``Timeout`` bounds.

    ``deadline_epoch_ms`` is when that ``Timeout`` is up at, as the host decided it when it
    began this phase; it is what the context reports as the time remaining.
    """
    request = _PREPARED["request"]
    library = _PREPARED["library"]
    entry = _PREPARED["entry"]
    request["deadlineEpochMs"] = deadline_epoch_ms

    event_text = request["event"]
    try:
        event = json.loads(event_text) if event_text else {}
    except ValueError as error:
        return _raised("Runtime.InvalidEvent", str(error), [])

    context = LambdaContext(request, request["environment"])

    try:
        returned = entry(event, context)
    except BaseException as error:
        trace = error.__traceback__
        # The frame this runtime called the handler from is not part of the handler's
        # own stack, so the trace starts at the handler.
        if trace is not None:
            trace = trace.tb_next
        return _raised(
            type(error).__name__,
            str(error),
            _frames(trace, library) if trace is not None else [],
        )

    try:
        payload = json.dumps(returned)
    except BaseException as error:
        return _raised(
            "Runtime.MarshalError",
            "Unable to marshal response: %s" % error,
            [],
        )
    return {"outcome": "returned", "payload": payload}


def _main_prepare(payload):
    try:
        request = json.loads(payload)
    except BaseException as error:
        return json.dumps(_raised("Runtime.InternalError", str(error), []))
    # Where this runtime's OWN modules are, which a frame of one of them names.
    library = _runtime_library(request.get("environment") or {})
    _PREPARED["library"] = library
    try:
        outcome = _prepare(request, library)
    except BaseException as error:
        return json.dumps(
            _raised(
                type(error).__name__,
                str(error),
                _frames(error.__traceback__, library),
            )
        )
    if outcome is not None:
        return json.dumps(outcome)
    # Nothing to report: the initialization is done and the host may begin the invoke
    # phase.
    return ""


def _bind(request):
    """Put ONE invocation's own request on the prepared environment.

    An execution environment is initialized once and then serves invocation after
    invocation, so what the handler is called with — the event, the request id, the ARN
    the call addressed, the client context, the tenant and the environment of the
    invocation — is bound here rather than by the initialization. The logging handler is
    re-installed with this invocation's request id, so a record carries the invocation
    that wrote it and not the one that happened to initialize the environment.
    """
    prepared = _PREPARED.get("request")
    if prepared is None:
        return _raised(
            "Runtime.InternalError",
            "the execution environment has no prepared request to bind an invocation on",
            [],
        )
    for member in ("event", "requestId", "invokedFunctionArn", "clientContext", "tenantId"):
        prepared[member] = request.get(member)
    environment = request.get("environment")
    if environment is not None:
        prepared["environment"] = environment
    _install_logging(prepared["requestId"])
    return None


def _main_bind(payload):
    library = _PREPARED.get("library") or ""
    try:
        request = json.loads(payload)
    except BaseException as error:
        return json.dumps(_raised("Runtime.InternalError", str(error), []))
    try:
        outcome = _bind(request)
    except BaseException as error:
        return json.dumps(
            _raised(
                type(error).__name__,
                str(error),
                _frames(error.__traceback__, library),
            )
        )
    if outcome is not None:
        return json.dumps(outcome)
    # Nothing to report: the invocation is bound and the host may begin the invoke phase.
    return ""


def _main_invoke(deadline_epoch_ms):
    library = _PREPARED.get("library") or ""
    try:
        return json.dumps(_invoke(deadline_epoch_ms))
    except BaseException as error:
        return json.dumps(
            _raised(
                type(error).__name__,
                str(error),
                _frames(error.__traceback__, library),
            )
        )


# The three functions this source leaves behind, all called by the HOST: the first runs the
# init phase, the second binds one invocation's own request onto the prepared environment,
# and the third runs the handler itself. Nothing above has run any of the caller's own code:
# the imports and definitions above are this runtime's own setup.
__lambda_prepare = _main_prepare
__lambda_bind = _main_bind
__lambda_invoke = _main_invoke
"##;
