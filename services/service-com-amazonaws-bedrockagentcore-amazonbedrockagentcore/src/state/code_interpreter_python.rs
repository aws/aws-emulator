// SPDX-License-Identifier: CC0-1.0

//! The PYTHON half of a code interpreter session's `executeCode`: the program the caller
//! submitted, RUN in the one contained interpreter this workspace embeds.
//!
//! `python` is one of the languages the built-in interpreter serves (source: smithy_query
//! query="shape" shape="ProgrammingLanguage", values python | javascript | typescript), so a
//! submitted Python program is compiled and executed, and what it printed, what it bound in
//! the session's namespace and the exception it raised are the program's own. Nothing about a
//! run is synthesised.
//!
//! # The engine is NOT this module's
//!
//! No second execution substrate is introduced here. The interpreter is
//! [`::service_com_amazonaws_lambda_awsgirapiservice::state::execution::python`] — the one
//! contained RustPython this workspace already holds for running caller-supplied Python,
//! driven exactly as the Amazon EMR notebook plane drives it
//! (`service-com-amazonaws-emr-elasticmapreduce`'s `state::notebook_run`). Everything that
//! CONTAINS and BOUNDS a run therefore lives in that one module and is maintained once: its
//! native modules are an allow-list with no socket, subprocess, memory-map, native-library or
//! thread module in it, its `posix` is that crate's own with every host-reaching call
//! refused, every way of opening a HOST file is taken away before any guest code runs, and
//! the deadline, traced-step, memory, recursion and output bounds are enforced in host code
//! the program cannot reach — including the latch that makes a stopped run's answer that
//! stop whatever the program catches.
//!
//! What this module adds is the code interpreter's own wiring, and nothing else:
//!
//! * the KERNEL ([`KERNEL_SOURCE`]) — ordinary Python, handed to the interpreter as the
//!   module the run resolves, the way the EMR kernel and the `pyspark` package this
//!   workspace supplies are: it executes the session's context programs and then the
//!   submitted one against ONE namespace, keeps the two output streams apart, and answers
//!   with them;
//! * the session WORKSPACE as the only filesystem the program sees: the files go IN with the
//!   run and the files it wrote come back OUT in its answer, so `open` reads and writes the
//!   same workspace the file tools and the command plane do. No host path is reachable —
//!   the engine refuses every host opener, and the `open` the program resolves is the
//!   kernel's own over an in-memory mapping;
//! * the host-side bounds around the run: it happens on a thread of its own with the stack a
//!   run is given, under the run slots of [`crate::ops`]'s own pool, and this module waits
//!   for its answer no longer than every span the engine spends on one run — standing the
//!   interpreter up, reading the program's imports, then the program's own wall clock — and a
//!   grace on top of them; past that the run has been given up on, its thread keeps its place
//!   in the pool until it really ends, and the call is answered rather than left waiting.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::python::{
    self, Budget, Completion, Stop,
};
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    ENVIRONMENT_BUILD_SPAN, INIT_PHASE_LIMIT,
};

use super::SandboxFilesystem;

/// The `ProgrammingLanguage` value this engine serves.
pub const LANGUAGE_PYTHON: &str = "python";

/// The `LanguageRuntime` value that runs Python (source: smithy_query query="shape"
/// shape="LanguageRuntime", values nodejs | deno | python).
pub const PYTHON_RUNTIME: &str = "python";

/// The module the kernel is handed to the interpreter as, and the entry point it resolves in
/// it.
const KERNEL_MODULE: &str = "code_interpreter_kernel.py";
/// See [`KERNEL_MODULE`].
const KERNEL_HANDLER: &str = "code_interpreter_kernel.execute";

/// The stack the thread one run happens on is given.
///
/// CHOSEN: the interpreter is recursive over the program's own syntax, so a run needs more
/// stack than a request thread carries; it is the same order as the stack this emulator gives
/// any other run of caller-supplied code.
const RUN_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How long one submitted program may run for.
///
/// An EMULATOR bound, not an AWS one: the program runs in this process, so a loop with no
/// exit would otherwise hold a thread for ever. Long enough for any computation a tool call
/// performs.
const RUN_WALL_CLOCK: Duration = Duration::from_secs(5);

/// How much longer than its own wall clock a run is waited for before it is given up on.
///
/// CHOSEN: a run stops at the next event its interpreter delivers after the deadline, and the
/// interpreter is then finalized, so the answer of a stopped run arrives a little after its
/// wall clock rather than at it. Past this the call is answered with the bound the run was
/// stopped on instead of waiting for a program that is not coming back.
const RUN_GRACE: Duration = Duration::from_secs(5);

/// How long this call waits for the thread one run happens on before it gives up on it.
///
/// DERIVED, not chosen: the program's own span is only the last of the spans the engine
/// spends on one run. Before a line of the program is read, the interpreter it runs in is
/// built for it ([`ENVIRONMENT_BUILD_SPAN`]) and the program's own imports are read in it
/// ([`INIT_PHASE_LIMIT`]) — work this emulator does on whatever host it is running on, which
/// the engine charges to neither the program's span nor any bound of its own. A wait of the
/// program's span alone would therefore give up on a run whose own bounds had not been
/// reached yet, and report a program that was still being stood up as one stopped for taking
/// too long, on a loaded host and never on an idle one.
const RUN_HOST_WAIT: Duration = ENVIRONMENT_BUILD_SPAN
    .saturating_add(INIT_PHASE_LIMIT)
    .saturating_add(RUN_WALL_CLOCK)
    .saturating_add(RUN_GRACE);

/// How many traced events one program may spend, how much memory it may add to this process,
/// how much of its output is kept, how large its answer may be and how deep it may recurse.
///
/// Every one of them is the EMULATOR's: a program is caller-supplied code, so what one tool
/// call may cost is stated here and enforced by the engine inside the run, in host code the
/// program cannot reach.
fn run_budget() -> Budget {
    Budget {
        wall_clock: RUN_WALL_CLOCK,
        steps: 20_000_000,
        memory_bytes: 256 * 1024 * 1024,
        // The directory a program writes its own files to is a disk on the real sandbox and
        // is held in this process's memory here, so what one run may leave there is bounded
        // like everything else it holds.
        ephemeral_bytes: 256 * 1024 * 1024,
        log_bytes: 64 * 1024,
        // The answer carries the two output streams and the files the program wrote, so it
        // is the response cap that bounds what one run can make this process hold.
        response_bytes: 4 * 1024 * 1024,
        recursion_limit: 2_000,
    }
}

/// The most bytes of the session workspace handed to one run.
///
/// The workspace may hold more ([`super::MAX_SESSION_FILE_BYTES`]), and the files go in with
/// the run, so this is what keeps one tool call's event proportional to the call: the files
/// past it are not handed to that run, which reads and writes the ones that were.
const MAX_WORKSPACE_BYTES_PER_RUN: usize = 4 * 1024 * 1024;

/// What one program run produced.
#[derive(Debug, Clone)]
pub struct PythonRun {
    pub stdout: String,
    pub stderr: String,
    /// `0` when the program ran to completion, the status a `SystemExit` named, and `1` for a
    /// program that ended on an uncaught exception or hit one of the run's bounds.
    pub exit_code: i32,
    /// How long the run took, which is what the result reports as its `executionTime`.
    pub elapsed: Duration,
    /// Which of the session's CONTEXT programs no longer run, by their position in the
    /// context this run was handed.
    ///
    /// A context program is re-executed to rebuild the namespace it left, and one that was
    /// fine when it ran may not be now — the file it opened has since been removed, say — so
    /// its failure is not this run's: it is reported here, the caller drops it from the
    /// session's context, and the submitted program runs regardless.
    pub failed_context: Vec<usize>,
}

/// Run `script` in the contained interpreter, on top of `context_scripts`, over
/// `filesystem` — the session's own workspace, which the program reads and writes through the
/// kernel's `open` and nothing else.
///
/// Nothing of the run happens on the async executor and nothing of it happens under this
/// service's state guard: the interpreter is synchronous and CPU-bound, so it runs on a
/// thread of its own and this call awaits its answer under a ceiling.
pub async fn run_python(
    context_scripts: &[String],
    script: &str,
    filesystem: &Arc<Mutex<SandboxFilesystem>>,
) -> PythonRun {
    let event = match run_event(context_scripts, script, filesystem) {
        Ok(event) => event,
        Err(reason) => return not_run(reason),
    };
    let budget = run_budget();
    let (sender, receiver) = ::tokio::sync::oneshot::channel();
    let spawned = ::std::thread::Builder::new()
        .name("code-interpreter-python".to_string())
        .stack_size(RUN_STACK_BYTES)
        .spawn(move || {
            let modules = vec![PackageModule {
                path: KERNEL_MODULE.to_string(),
                source: KERNEL_SOURCE.to_string(),
            }];
            let finished = python::run(&python::Run {
                modules: &modules,
                handler: KERNEL_HANDLER,
                // The program sees no variable of this process and none of another
                // session: a code interpreter session is given no environment of its own.
                environment: &BTreeMap::new(),
                // The instruction set the environment the program runs in has, which the
                // run reports as its own machine. A session names none, so it is the one
                // the engine's own runtime reports when none is named.
                architecture:
                    ::service_com_amazonaws_lambda_awsgirapiservice::state::DEFAULT_ARCHITECTURE,
                event: Some(&event),
                request_id: "",
                invoked_function_arn: "",
                client_context: None,
                tenant_id: None,
                // Nor an Amazon Cognito identity: a session's program is this service's own
                // run, made by no identity a pool vended credentials to.
                cognito_identity: None,
                budget,
                // The program reaches no other service of this emulator: this run is given
                // no seam to dispatch an AWS call on.
                aws: None,
                // It is no Lambda durable execution's run either, so it takes no step of one
                // and checkpoints nothing onto one.
                durable: None,
                // And it mounts no Amazon EFS file system: the files the program reaches are
                // the session's own, which this service holds.
                file_systems: &[],
            });
            let _ = sender.send(finished);
        });
    if spawned.is_err() {
        return not_run("the program could not be given a thread to run on".to_string());
    }
    let waited = ::tokio::time::timeout(RUN_HOST_WAIT, receiver).await;
    let finished = match waited {
        Ok(Ok(finished)) => finished,
        // The run was given up on, or its thread ended without reporting: its own thread
        // keeps the run slot until it really ends, and the caller is answered now. Nothing is
        // known about what the program came to, so no bound of its own is reported for it.
        Ok(Err(_)) | Err(_) => {
            return not_run(
                "The program was given up on: none of its answer was read.".to_string(),
            )
        }
    };
    let elapsed = finished.elapsed;
    match &finished.completion {
        // The engine's LATCH: a run whose budget was spent is a stopped run whatever the
        // program went on to return or catch, so the bound is what this reports.
        Completion::Stopped(stop) => PythonRun {
            stdout: String::new(),
            stderr: format!("{}\n", stopped_reason(*stop)),
            exit_code: 1,
            elapsed,
            failed_context: Vec::new(),
        },
        Completion::Returned(answer) => answered(answer, elapsed, filesystem),
        // The kernel reports the program's own exception itself, so one that reaches here is
        // the kernel's or the interpreter's rather than the program's.
        Completion::Raised {
            error_type,
            error_message,
            ..
        } => PythonRun {
            stdout: String::new(),
            stderr: format!("{error_type}: {error_message}\n"),
            exit_code: 1,
            elapsed,
            failed_context: Vec::new(),
        },
        Completion::Internal(reason) => not_run(reason.clone()),
        Completion::Streamed { .. } => {
            not_run("the program's answer was not the one the kernel reports".to_string())
        }
    }
}

/// What a run that did not happen reports: the reason, and no output, status or duration for
/// work nothing performed.
fn not_run(reason: String) -> PythonRun {
    PythonRun {
        stdout: String::new(),
        stderr: format!("{reason}\n"),
        exit_code: 1,
        elapsed: Duration::ZERO,
        failed_context: Vec::new(),
    }
}

/// How a run the engine STOPPED is reported, in the words of the bound it ran past.
fn stopped_reason(stop: Stop) -> String {
    match stop {
        Stop::Deadline | Stop::HostCeiling => format!(
            "The program was stopped after {} seconds.",
            RUN_WALL_CLOCK.as_secs()
        ),
        Stop::Steps => "The program was stopped after spending its step budget.".to_string(),
        Stop::Memory => "The program was stopped after taking more memory than one run may.".to_string(),
        Stop::Response => {
            "The program was stopped: its output and the files it wrote are larger than one \
             run may answer with."
                .to_string()
        }
    }
}

/// The EVENT one run is handed: the session's context programs, the submitted program, and
/// the workspace the program reads and writes.
///
/// Every file goes in as base64, so a workspace entry whose bytes are not text reaches the
/// program exactly as it is stored and comes back the same way.
fn run_event(
    context_scripts: &[String],
    script: &str,
    filesystem: &Arc<Mutex<SandboxFilesystem>>,
) -> Result<String, String> {
    let held = filesystem.lock().unwrap_or_else(PoisonError::into_inner);
    let mut files = ::serde_json::Map::new();
    let mut handed = 0usize;
    for (key, bytes) in held.files() {
        handed = handed.saturating_add(bytes.len());
        if handed > MAX_WORKSPACE_BYTES_PER_RUN {
            break;
        }
        files.insert(
            key,
            ::serde_json::Value::String(::aws_smithy_types::base64::encode(&bytes)),
        );
    }
    drop(held);
    let event = ::serde_json::json!({
        "context": context_scripts,
        "code": script,
        "files": files,
    });
    ::serde_json::to_string(&event)
        .map_err(|error| format!("the program's own workspace could not be handed to it: {error}"))
}

/// What the kernel ANSWERED, as the run this emulator reports — and the files the program
/// wrote, back into the session's workspace.
fn answered(
    answer: &str,
    elapsed: Duration,
    filesystem: &Arc<Mutex<SandboxFilesystem>>,
) -> PythonRun {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(answer) else {
        return not_run("the program's answer could not be read".to_string());
    };
    let text = |member: &str| {
        document
            .get(member)
            .and_then(::serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut held = filesystem.lock().unwrap_or_else(PoisonError::into_inner);
    // The paths are the PROGRAM's, so each one is normalized and weighed by the workspace
    // itself ([`SandboxFilesystem`]): a path that does not address the session's own
    // workspace, and a write it has no room for, is refused here exactly as a `writeFiles`
    // call's would be — and said so on the run's error stream rather than left looking
    // stored.
    let mut refused = 0usize;
    for path in document
        .get("removed")
        .and_then(::serde_json::Value::as_array)
        .unwrap_or(&Vec::new())
    {
        if let Some(path) = path.as_str() {
            let _ = held.remove(path);
        }
    }
    if let Some(written) = document
        .get("written")
        .and_then(::serde_json::Value::as_object)
    {
        for (path, encoded) in written {
            let refusal = encoded
                .as_str()
                .ok_or(())
                .and_then(|encoded| ::aws_smithy_types::base64::decode(encoded).map_err(|_| ()))
                .and_then(|bytes| held.write(path, bytes).map_err(|_| ()));
            if refusal.is_err() {
                refused = refused.saturating_add(1);
            }
        }
    }
    drop(held);
    let failed_context = document
        .get("failedContext")
        .and_then(::serde_json::Value::as_array)
        .map(|positions| {
            positions
                .iter()
                .filter_map(::serde_json::Value::as_u64)
                .filter_map(|position| usize::try_from(position).ok())
                .collect()
        })
        .unwrap_or_default();
    let mut stderr = text("stderr");
    if refused > 0 {
        stderr.push_str(&format!(
            "The session workspace had no room for {refused} of the files this run wrote, so \
             they were not stored.\n"
        ));
    }
    PythonRun {
        stdout: text("stdout"),
        stderr,
        exit_code: document
            .get("exitCode")
            .and_then(::serde_json::Value::as_i64)
            .and_then(|status| i32::try_from(status).ok())
            .unwrap_or(0),
        elapsed,
        failed_context,
    }
}

/// The KERNEL one run resolves: the program that executes the session's context programs and
/// then the submitted one against ONE namespace, over the session's workspace, and answers
/// with the two output streams, the status and the files the program wrote.
///
/// Ordinary Python, the way every module this workspace supplies an interpreter is (the EMR
/// notebook kernel, the `pyspark` package). It is NOT the containment — that is the engine's,
/// in host code the program cannot reach — it is the SESSION: one namespace, so a name an
/// earlier run bound is a name a later one sees; two streams kept apart, because the result
/// has a member for each; and a filesystem that is the session's workspace and nothing else,
/// an in-memory mapping this kernel was handed, with no path of the host in it and no way to
/// reach one.
const KERNEL_SOURCE: &str = r#"
"""The session one code interpreter run happens in."""

import base64
import io
import sys
import traceback


def _files_of(event):
    """The session workspace as the run was handed it, keyed by its own paths."""
    held = {}
    for path, encoded in (event.get("files") or {}).items():
        held[path] = base64.b64decode(encoded)
    return held


def _opener(files, written, removed):
    """`open` over the session's workspace: the only filesystem a run sees."""
    def path_of(named):
        # The workspace IS the root of the filesystem a run sees, so a leading separator
        # addresses that root rather than a host path.
        if isinstance(named, bytes):
            named = named.decode("utf-8", "surrogateescape")
        elif not isinstance(named, str):
            held = getattr(named, "__fspath__", None)
            if held is None:
                raise TypeError(
                    "expected str, bytes or os.PathLike object, not " + type(named).__name__
                )
            return path_of(held())
        return named.lstrip("/")

    class WorkspaceFile:
        """One open file of the workspace, over an in-memory stream."""

        def __init__(self, path, stream, mode, binary, encoding, errors):
            self._path = path
            self._stream = stream
            self._binary = binary
            self._encoding = encoding
            self._errors = errors
            self._writing = any(flag in mode for flag in ("w", "a", "x", "+"))
            self._closed = False
            self.name = path
            self.mode = mode

        def __getattr__(self, name):
            if name.startswith("_"):
                raise AttributeError(name)
            return getattr(self._stream, name)

        def __iter__(self):
            return iter(self._stream)

        def __next__(self):
            return next(self._stream)

        def __enter__(self):
            return self

        def __exit__(self, kind, value, traceback_):
            self.close()
            return False

        @property
        def closed(self):
            return self._closed

        def flush(self):
            if self._closed or not self._writing:
                return None
            held = self._stream.getvalue()
            if not self._binary:
                held = held.encode(self._encoding, self._errors)
            files[self._path] = held
            written[self._path] = held
            if self._path in removed:
                removed.remove(self._path)
            return None

        def close(self):
            if self._closed:
                return None
            self.flush()
            self._closed = True
            return None

    def holds_directory(path):
        under = path.rstrip("/") + "/"
        return any(held.startswith(under) for held in files)

    def open_file(
        file,
        mode="r",
        buffering=-1,
        encoding=None,
        errors=None,
        newline=None,
        closefd=True,
        opener=None,
    ):
        path = path_of(file)
        binary = "b" in mode
        creating = "x" in mode
        truncating = "w" in mode
        text_encoding = encoding or "utf-8"
        text_errors = errors or "strict"
        held = files.get(path)
        if held is None and not (truncating or creating or "a" in mode):
            if holds_directory(path):
                raise IsADirectoryError(21, "Is a directory", path)
            raise FileNotFoundError(2, "No such file or directory", path)
        if creating and held is not None:
            raise FileExistsError(17, "File exists", path)
        initial = b"" if held is None or truncating else held
        if binary:
            stream = io.BytesIO(initial)
        else:
            stream = io.StringIO(initial.decode(text_encoding, text_errors))
        opened = WorkspaceFile(path, stream, mode, binary, text_encoding, text_errors)
        if "a" in mode:
            stream.seek(0, 2)
        if truncating or creating:
            # A file opened for writing EXISTS from the instant it is opened, as it does on
            # a real filesystem.
            opened.flush()
        return opened

    def remove_file(path):
        path = path_of(path)
        if path not in files:
            raise FileNotFoundError(2, "No such file or directory", path)
        del files[path]
        written.pop(path, None)
        if path not in removed:
            removed.append(path)
        return None

    return open_file, remove_file


def execute(event, context):
    files = _files_of(event)
    written = {}
    removed = []
    open_file, remove_file = _opener(files, written, removed)
    out = io.StringIO()
    err = io.StringIO()
    namespace = {
        "__name__": "__main__",
        # `open` is the name a program resolves for the session's own filesystem. It is the
        # kernel's, over the mapping above: nothing of the host is reachable through it, and
        # the engine has already taken away every way of opening a file of the host.
        "open": open_file,
    }
    held_out, held_err = sys.stdout, sys.stderr
    sys.stdout, sys.stderr = out, err
    status = 0
    failed = []
    try:
        # The context programs rebuild the namespace earlier runs left. One that no longer
        # runs is reported rather than failing this run, and its output is not this run's:
        # the streams are emptied before the submitted program runs.
        for index, source in enumerate(event.get("context") or []):
            try:
                exec(compile(source, "<executeCode>", "exec"), namespace)
            except BaseException:
                failed.append(index)
        out.seek(0)
        out.truncate(0)
        err.seek(0)
        err.truncate(0)
        try:
            exec(compile(event.get("code") or "", "<executeCode>", "exec"), namespace)
        except SystemExit as ended:
            code = ended.code
            if code is None:
                status = 0
            elif isinstance(code, int):
                status = code
            else:
                err.write(str(code) + "\n")
                status = 1
        except BaseException:
            err.write(traceback.format_exc())
            status = 1
    finally:
        sys.stdout, sys.stderr = held_out, held_err
    return {
        "stdout": out.getvalue(),
        "stderr": err.getvalue(),
        "exitCode": status,
        "written": {
            path: base64.b64encode(held).decode("ascii") for path, held in written.items()
        },
        "removed": removed,
        "failedContext": failed,
    }
"#;
