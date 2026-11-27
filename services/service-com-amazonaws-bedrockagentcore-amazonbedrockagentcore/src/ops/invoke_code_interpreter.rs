// SPDX-License-Identifier: CC0-1.0

//! @op InvokeCodeInterpreter
//!
//! `POST /code-interpreters/{codeInterpreterIdentifier}/tools/invoke -> 200`. Invokes
//! one tool on a live code interpreter session and answers with the tool's result as a
//! `vnd.amazon.eventstream` body: a single `result` event carrying the
//! `CodeInterpreterResult` the tool produced.
//!
//! This operation is hand-authored rather than generated: its output member `stream`
//! targets a `@streaming` union, which the op-stub generator declines
//! (`src/ops/SKIPPED_STUBS.md`) and whose generated `ErasedOperation` shim refuses the
//! request. The framing and the routing seam live in `service_common::event_stream`
//! and `src/service.rs`; this file holds the behaviour, and the shapes are in
//! `src/ops/types/invoke_code_interpreter_types.rs`.
//!
//! # What each tool does here
//!
//! The session named by `x-amzn-code-interpreter-session-id` owns a WORKSPACE and an
//! execution CONTEXT in [`crate::state`], and every tool below computes its answer
//! from them:
//!
//! * `writeFiles`, `readFiles`, `listFiles`, `removeFiles` — the session's workspace.
//!   A path is a workspace-relative key ([`crate::state::normalize_session_path`]), so
//!   no tool argument names anything outside the session's own state, and a write is
//!   bounded by the workspace ceilings the store declares.
//! * `executeCode` with `language: javascript` — the submitted script RUNS, in the
//!   QuickJS engine embedded in this process (`rquickjs`), and what it wrote to
//!   `console` is the `stdout` / `stderr` the result reports. A run is evaluated on top
//!   of the scripts this session has already run to completion, which is what makes
//!   `clearContext` mean something: with it set, the context is dropped first.
//! * `executeCode` with `language: python` — the submitted program RUNS too, in the
//!   contained Python interpreter this crate embeds (`crate::state::run_python`), over
//!   the same session workspace the file tools read and write, and on top of the same
//!   per-language context.
//! * `executeCommand` and `startCommandExecution` — the command line is PERFORMED, by
//!   the bounded POSIX shell the workspace drives for caller-supplied shell text
//!   (`crate::state::run_command`), over that same session workspace: so the file
//!   tools, a run and a command are three views of ONE filesystem. The synchronous tool
//!   answers with the two streams and the exit status; the asynchronous one mints a task
//!   ([`crate::state::CommandTaskRecord`]) that `getTask` reports and `stopTask`
//!   cancels.
//! * `executeCode` with any other `language` — this emulator embeds no engine for it
//!   (there is no TypeScript transpiler here), so the result says the work was NOT run,
//!   with `isError` set. Nothing is synthesised to look as though it ran: no output, no
//!   exit status and no duration is reported for work no engine performed.
//!
//! Which members the `result` carries follows from the tool, not from the call:
//! `structuredContent` reports a RUN (its members are `stdout`, `stderr`, `exitCode`,
//! `executionTime`, `taskId`, `taskStatus`), so a tool that performed no run — a
//! workspace tool, or work no engine here performs — has nothing to put in it. `isError`
//! is the result's own success flag: it is documented as `true` when "the content
//! contains error messages or exception information" and `false` when "the content
//! contains successful execution results", so EVERY result reports it, `false`
//! included. [`ToolOutcome`] is where that lives.
//!
//! # A tool that could not do what was asked
//!
//! A failure of the invoked TOOL — a path that does not address the session workspace, a
//! file the workspace does not hold, a workspace with no room left, a task id that names
//! none — is reported IN BAND, on a 200 whose `result` carries `isError` and a text block
//! beginning `Error executing tool ` and the tool's name ([`ToolOutcome::Failed`]). It is
//! not a modelled API error: `isError` is documented as indicating that "the result
//! represents an error", which is what a tool-level failure is, and a client that handles
//! one keeps using its session. The modelled errors below are for the REQUEST — the
//! interpreter, the session and the arguments a tool cannot be invoked without at all.
//!
//! # How a run is bounded and contained
//!
//! The engine is an execution substrate, so every limit lives in this host code, which
//! a script cannot reach:
//!
//! * wall clock — the engine's interrupt handler stops the script once
//!   [`RUN_WALL_CLOCK`] has passed, which is what bounds a CPU-bound loop a timeout
//!   alone could not preempt;
//! * memory and stack — [`RUN_MEMORY_BYTES`] and [`RUN_STACK_BYTES`] are set on the
//!   engine before anything is evaluated, so an allocation or a recursion past them
//!   ends that run and nothing else;
//! * output — each captured stream stops at [`MAX_CAPTURED_STREAM_BYTES`], so a small
//!   request cannot make a large response, and a `readFiles` result stops at
//!   [`MAX_READ_RESULT_BYTES`] over at most [`MAX_TOOL_PATHS`] files, so neither can a
//!   tool call that names many large files;
//! * script size — a submitted script past [`MAX_SCRIPT_BYTES`] is not compiled;
//! * concurrency — at most [`MAX_CONCURRENT_RUNS`] scripts run at once, so the memory
//!   and CPU the engine can hold is bounded however many calls arrive, and a call that
//!   waits [`PERMIT_WAIT`] for its turn is answered with the modelled throttle;
//! * the run happens on a blocking thread (`tokio::task::spawn_blocking`), never on an
//!   async worker, and with NO state guard of this service held, so one caller's script
//!   cannot stall another caller's request.
//!
//! Containment: the engine is built with default features only — no module loader and
//! no native module loading — so a script can import nothing, and plain QuickJS
//! exposes no filesystem, process, environment or socket API at all. The only host
//! functions a script can call are the two that collect its console output. The
//! session workspace is reachable through the file tools, never from inside a script,
//! and it lives in this process's memory: no host path is opened for it.
//!
//! ## Input
//!
//! * `codeInterpreterIdentifier` (String, `@httpLabel`) **required**
//! * `sessionId` (CodeInterpreterSessionId, pattern `^[0-9a-zA-Z]{1,40}$`,
//!   `@httpHeader("x-amzn-code-interpreter-session-id")`) — the session to run in; a
//!   call that names none is served in a session this call starts, which the response
//!   names back in the same header.
//! * `name` (ToolName) **required** — `executeCode` | `executeCommand` | `readFiles` |
//!   `listFiles` | `removeFiles` | `writeFiles` | `startCommandExecution` | `getTask` |
//!   `stopTask`.
//! * `arguments` (ToolArguments) — the invoked tool's parameters.
//! * `traceId`, `traceParent` (`@httpHeader`) — distributed tracing; part of no answer.
//!
//! ## Output
//!
//! * `stream` (CodeInterpreterStreamOutput, `@httpPayload`, `@streaming`) — the body:
//!   one `result` event holding `content` (required), `structuredContent` and
//!   `isError`.
//! * `sessionId` (`@httpHeader("x-amzn-code-interpreter-session-id")`) — the session
//!   the tool ran in.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — the session named is not one this
//!   emulator holds for that code interpreter.
//! * `ValidationException` (HTTP 400) — the session named is `TERMINATED` (stopped, or
//!   past its time-to-live), so it is no longer a session a tool can run in; or the
//!   invoked tool was given no argument it cannot work without.
//! * `AccessDeniedException` (403), `InternalServerException` (500),
//!   `ServiceQuotaExceededException` (402), `ThrottlingException` (429).

#[allow(unused_imports)]
use super::types::*;
use aws_smithy_runtime_api::http::{Request, Response};
use rquickjs::function::Func;
use rquickjs::{CatchResultExt, Context, Ctx, Runtime, Value};
use service_common::event_stream::{
    deserialize_request, error_response, event_stream_response, frame_event,
};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the
/// model declares for it.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentcore#InvokeCodeInterpreter")]
#[derive(Debug)]
pub enum InvokeCodeInterpreterErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationRefusal),
}

/// How long one script may run before the engine's interrupt handler stops it.
///
/// An EMULATOR bound, not an AWS one: the script runs in this process, so a loop with
/// no exit would otherwise hold a thread for ever. Long enough for any computation a
/// test or an agent performs in one tool call.
const RUN_WALL_CLOCK: Duration = Duration::from_secs(2);

/// The heap one run may use, set on the engine before anything is evaluated.
const RUN_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// The JavaScript stack one run may use. Well inside the stack of the blocking thread
/// the run happens on, so deep recursion in a script ends that run with the engine's
/// own stack-overflow exception rather than touching the thread's own guard page.
const RUN_STACK_BYTES: usize = 512 * 1024;

/// The most bytes of one output stream a run reports back. Past it the stream keeps
/// what it already captured and the rest is dropped, so a one-line script cannot make
/// an unbounded response.
const MAX_CAPTURED_STREAM_BYTES: usize = 64 * 1024;

/// The longest script this engine compiles. `MaxLenString` admits 100 MB of `code`,
/// which no in-process engine should be handed; this is the EMULATOR's own bound.
const MAX_SCRIPT_BYTES: usize = 1024 * 1024;

/// The most files one tool call may address. `StringList` and `InputContentBlockList`
/// carry no `@length`, so this is the EMULATOR's own bound on the COUNT side of a tool
/// call: it is what keeps the work and the response of one call proportional to the
/// call rather than to the product of a caller-chosen count and a caller-chosen file
/// size.
const MAX_TOOL_PATHS: usize = 64;

/// The most bytes one `readFiles` result carries across every block in it. The cap on
/// the PRODUCT the count bound above cannot give on its own, since one workspace file
/// may hold [`crate::state::MAX_SESSION_FILE_BYTES`]: a read whose result would pass
/// this is refused rather than materialised.
const MAX_READ_RESULT_BYTES: usize = 4 * 1024 * 1024;

/// How many scripts this service runs at once. Together with [`RUN_MEMORY_BYTES`] it
/// bounds what the engine can hold however many calls arrive at once, and it keeps the
/// blocking pool from filling with CPU-bound runs.
const MAX_CONCURRENT_RUNS: usize = 4;

/// How long a call waits for one of those run slots before it is answered with the
/// modelled `ThrottlingException` instead of queueing behind other callers' scripts.
const PERMIT_WAIT: Duration = Duration::from_secs(5);

/// The `ProgrammingLanguage` value this emulator has an engine for (source:
/// smithy_query query="shape" shape="ProgrammingLanguage", values python | javascript |
/// typescript).
const LANGUAGE_JAVASCRIPT: &str = "javascript";

/// The `LanguageRuntime` values that run JavaScript, which is what the embedded engine
/// serves (source: smithy_query query="shape" shape="LanguageRuntime", values nodejs |
/// deno | python). A request naming any other runtime — including one this list does
/// not know — is answered as work nothing ran, rather than run as JavaScript on a
/// guess.
const JAVASCRIPT_RUNTIMES: [&str; 2] = ["nodejs", "deno"];

/// `ContentBlockType`'s values, as a tool result's blocks use them (source:
/// smithy_query query="shape" shape="ContentBlockType", values text | image | resource
/// | resource_link).
const BLOCK_TEXT: &str = "text";
/// See [`BLOCK_TEXT`].
const BLOCK_RESOURCE: &str = "resource";
/// See [`BLOCK_TEXT`].
const BLOCK_RESOURCE_LINK: &str = "resource_link";

/// `ResourceContentType`'s values (source: smithy_query query="shape"
/// shape="ResourceContentType", values text | blob).
const RESOURCE_TEXT: &str = "text";
/// See [`RESOURCE_TEXT`].
const RESOURCE_BLOB: &str = "blob";

/// The media type reported for a workspace entry that is a directory rather than a
/// file, which is how a `listFiles` block tells the two apart.
const DIRECTORY_MEDIA_TYPE: &str = "inode/directory";

/// Milliseconds in one second, the unit `ToolResultStructuredContent$executionTime`
/// states a duration in.
const MILLISECONDS_PER_SECOND: f64 = 1_000.0;

/// This operation's own modelled 400 `ValidationException`, for a tool argument the
/// invoked tool cannot work without or cannot work with.
///
/// `reason` is `ValidationExceptionReason`'s `FieldValidationFailed` (source:
/// smithy_query query="shape" shape="ValidationExceptionReason", values CannotParse |
/// FieldValidationFailed | IdempotentParameterMismatchException | EventInOtherSession |
/// ResourceConflict), which is the member-level failure every case here is.
fn validation(message: impl Into<String>) -> InvokeCodeInterpreterErrorModeled {
    InvokeCodeInterpreterErrorModeled::ValidationException(ValidationRefusal::Field(
        ValidationException {
            message: message.into(),
            reason: crate::service::validation::FIELD_VALIDATION_FAILED.to_string(),
            field_list: None,
        },
    ))
}

/// This operation's own modelled 400 `ValidationException`, for a session a tool can no
/// longer run in.
///
/// The body carries `message` alone ([`ValidationRefusal::resource_state`]): the request's
/// members are all well formed — the session they address has stopped — so there is no
/// member for `reason` to classify or for `fieldList` to name. The sibling browser plane
/// reports the identical condition the same way.
fn session_not_active(message: impl Into<String>) -> InvokeCodeInterpreterErrorModeled {
    InvokeCodeInterpreterErrorModeled::ValidationException(ValidationRefusal::resource_state(
        message,
    ))
}

fn internal(message: impl Into<String>) -> InvokeCodeInterpreterErrorModeled {
    InvokeCodeInterpreterErrorModeled::InternalServerException(InternalServerException {
        message: Some(message.into()),
    })
}

fn throttling(message: impl Into<String>) -> InvokeCodeInterpreterErrorModeled {
    InvokeCodeInterpreterErrorModeled::ThrottlingException(ThrottlingException {
        message: Some(message.into()),
    })
}

/// The `InvokeCodeInterpreter` operation.
///
/// Owns the shared engine (the constructor seam). The generated routing table holds
/// one of these and `service.rs` holds another wrapped in an `EventStreamRoute`; both
/// reach the same state, because the state lives in the shared engine, not on the
/// operation.
pub struct InvokeCodeInterpreter<Eng> {
    engine: Arc<Eng>,
    /// The run slots [`MAX_CONCURRENT_RUNS`] declares, held by the service rather than
    /// by one request, so the bound is over every caller of this endpoint.
    runs: Arc<::tokio::sync::Semaphore>,
}

impl<Eng> InvokeCodeInterpreter<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            engine,
            runs: Arc::new(::tokio::sync::Semaphore::new(MAX_CONCURRENT_RUNS)),
        }
    }
}

impl<Eng: Send + Sync + 'static> InvokeCodeInterpreter<Eng> {
    /// The whole request: resolve the session, invoke the named tool on it, and frame
    /// the tool's result as the operation's one `result` event.
    async fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<Response, InvokeCodeInterpreterErrorModeled> {
        let input: InvokeCodeInterpreterInput =
            deserialize_request(protocol, req, &InvokeCodeInterpreterInput::SCHEMA)
                .map_err(validation)?;

        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(|| internal("the shared engine does not carry this service's state"))?;

        // A code interpreter is created through the CONTROL plane (apart from the one the
        // service provides to every account), so this call resolves the interpreter before
        // it reaches any session of it: a tool cannot run in a sandbox that does not
        // exist, and a call that names no session would otherwise START one on it. The
        // inventory is read and released before this service's own store is taken.
        let held = crate::state::holds_code_interpreter(
            &*self.engine,
            &input.code_interpreter_identifier,
        )
        .ok_or_else(|| internal("the shared engine does not carry the AgentCore resource inventory"))?;
        if !held {
            return Err(
                InvokeCodeInterpreterErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::resource_not_found_message(
                            crate::state::NotFoundResource::CodeInterpreter,
                            &input.code_interpreter_identifier,
                        )),
                    },
                ),
            );
        }

        let now = SystemTime::now();
        // The session this tool runs in, resolved (or started) under the guard and
        // then addressed by id, so nothing below holds the guard while a script runs.
        let session_id = {
            let mut store = state.lock();
            match input.session_id.as_deref() {
                Some(named) => {
                    let session = store
                        .code_interpreter_session(&input.code_interpreter_identifier, named)
                        .ok_or_else(|| {
                            InvokeCodeInterpreterErrorModeled::ResourceNotFoundException(
                                ResourceNotFoundException {
                                    message: Some(format!(
                                        "session {named} not found for code interpreter {}",
                                        input.code_interpreter_identifier
                                    )),
                                },
                            )
                        })?;
                    // A session that "has expired or been stopped" cannot serve a tool
                    // call, and it can never come back: the `sessionId` the request
                    // carries no longer addresses anything a tool can run in, which is a
                    // member the caller has to change rather than a conflict it can
                    // resolve and retry.
                    let status = session.status_as_of(now);
                    if status == crate::state::CODE_INTERPRETER_SESSION_TERMINATED {
                        return Err(session_not_active(
                            crate::state::code_interpreter_session_not_active(named),
                        ));
                    }
                    named.to_string()
                }
                // `sessionId` is optional on the request and bound on the RESPONSE, so
                // a call that names no session is served in one this call starts, with
                // the modelled default time-to-live, and the response names it back.
                None => {
                    let seq = store.next_seq();
                    let started = crate::state::CodeInterpreterSessionRecord::started(
                        seq,
                        input.code_interpreter_identifier.clone(),
                        None,
                        crate::state::DEFAULT_CODE_INTERPRETER_SESSION_TIMEOUT_SECONDS,
                        None,
                        None,
                        now,
                    );
                    let session_id = started.session_id.clone();
                    store.insert_code_interpreter_session(started, None);
                    session_id
                }
            }
        };
        // A call that named no session has opened one, so the gauge that reports the
        // caller's live sessions is sampled now, with no store lock held; a call that named
        // one left the count where it was and the sampler publishes nothing new.
        crate::state::report_active_sessions(&self.engine);

        let arguments = input.arguments.clone().unwrap_or_default();
        // Every task of this session whose command the CLOCK has made due is performed
        // here, before the invoked tool reads anything: a task converges by being
        // materialised, so the file a submitted command wrote is in the workspace for
        // whichever tool looks next, not only for a `getTask`.
        self.converge_due_tasks(
            &state,
            &input.code_interpreter_identifier,
            &session_id,
        )
        .await?;
        // `ToolName` is a closed enum the request walk already checked the value
        // against, so the final arm is unreachable for a served request — it is here
        // because a match over a string has to be total.
        let outcome = match input.name.as_str() {
            "executeCode" => {
                self.execute_code(
                    &state,
                    &input.code_interpreter_identifier,
                    &session_id,
                    &arguments,
                )
                .await
            }
            "writeFiles" => self.write_files_in_session(
                &state,
                &input.code_interpreter_identifier,
                &session_id,
                &arguments,
                SystemTime::now(),
            )?,
            "readFiles" => self.with_session(
                &state,
                &input.code_interpreter_identifier,
                &session_id,
                |session| read_files(session, &arguments),
            )?,
            "listFiles" => self.with_session(
                &state,
                &input.code_interpreter_identifier,
                &session_id,
                |session| list_files(session, &arguments),
            )?,
            "removeFiles" => self.with_session_mut(
                &state,
                &input.code_interpreter_identifier,
                &session_id,
                |session| remove_files(session, &arguments),
            )?,
            "executeCommand" => {
                self.execute_command(
                    &state,
                    &input.code_interpreter_identifier,
                    &session_id,
                    &arguments,
                )
                .await
            }
            "startCommandExecution" => self.start_command_execution(
                &state,
                &input.code_interpreter_identifier,
                &session_id,
                &arguments,
            ),
            "getTask" => self.report_task(
                &state,
                &input.code_interpreter_identifier,
                &session_id,
                &arguments,
            ),
            "stopTask" => self.stop_task(
                &state,
                &input.code_interpreter_identifier,
                &session_id,
                &arguments,
            ),
            other => return Err(validation(format!("{other} is not a code interpreter tool."))),
        };
        let result = outcome?.into_result();

        let frames = frame_event(
            protocol,
            // The `CodeInterpreterStreamOutput` member this event is, which the frame
            // carries as its `:event-type` (source: smithy_query query="shape"
            // shape="CodeInterpreterStreamOutput").
            "result",
            false,
            &result,
            &CodeInterpreterResult::SCHEMA,
        )
        .map_err(|error| internal(format!("the result event could not be framed: {error}")))?;

        event_stream_response(
            protocol,
            "InvokeCodeInterpreter",
            &InvokeCodeInterpreterHeaders {
                session_id: Some(session_id),
            },
            &InvokeCodeInterpreterHeaders::SCHEMA,
            frames,
        )
        .map_err(|error| internal(format!("the response could not be serialized: {error}")))
    }

    /// Read the live session under the guard, hand it to `tool`, and release the guard
    /// with the tool's answer. The session is re-resolved here because it is addressed
    /// by id: a stop that landed in between is a 404 for this call, not a panic.
    fn with_session<T>(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        tool: impl FnOnce(&crate::state::CodeInterpreterSessionRecord) -> T,
    ) -> Result<T, InvokeCodeInterpreterErrorModeled> {
        let store = state.lock();
        let session = store
            .code_interpreter_session(code_interpreter_identifier, session_id)
            .ok_or_else(|| session_gone(code_interpreter_identifier, session_id))?;
        Ok(tool(session))
    }

    /// [`Self::with_session`] for a tool that CHANGES the session — one of the file
    /// tools that writes the workspace.
    fn with_session_mut<T>(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        tool: impl FnOnce(&mut crate::state::CodeInterpreterSessionRecord) -> T,
    ) -> Result<T, InvokeCodeInterpreterErrorModeled> {
        let mut store = state.lock();
        let session = store
            .code_interpreter_session_mut(code_interpreter_identifier, session_id)
            .ok_or_else(|| session_gone(code_interpreter_identifier, session_id))?;
        Ok(tool(session))
    }

    /// [`Self::with_session_mut`] for the ONE tool that GROWS the store: the bytes every
    /// other session holds are read under the same guard and handed to the write, so a
    /// workspace write holds the whole-store ceiling as well as this session's own.
    fn write_files_in_session(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        arguments: &ToolArguments,
        now: SystemTime,
    ) -> Result<Result<ToolOutcome, InvokeCodeInterpreterErrorModeled>, InvokeCodeInterpreterErrorModeled>
    {
        let mut store = state.lock();
        let elsewhere = store.session_bytes_elsewhere(code_interpreter_identifier, session_id);
        let session = store
            .code_interpreter_session_mut(code_interpreter_identifier, session_id)
            .ok_or_else(|| session_gone(code_interpreter_identifier, session_id))?;
        Ok(write_files(session, arguments, now, elsewhere))
    }

    /// `executeCode`: run the submitted program in the engine this emulator embeds for
    /// the language it names, on top of the session's execution context for that
    /// language and over the session's own workspace.
    async fn execute_code(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        arguments: &ToolArguments,
    ) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
        let Some(script) = arguments.code.clone() else {
            return Err(validation(
                "The executeCode tool requires the code argument.",
            ));
        };
        let Some(language) = arguments.language.clone() else {
            return Err(validation(
                "The executeCode tool requires the language argument.",
            ));
        };
        // `language` names what the program IS and `runtime` what it is to run under, so
        // the engine follows from the language and a runtime that cannot run that
        // language is a tool call that does not describe a run at all.
        let runtime = arguments.runtime.as_deref();
        let engine = match language.as_str() {
            LANGUAGE_JAVASCRIPT
                if runtime.is_none_or(|named| JAVASCRIPT_RUNTIMES.contains(&named)) =>
            {
                CodeEngine::JavaScript
            }
            crate::state::LANGUAGE_PYTHON
                if runtime.is_none_or(|named| named == crate::state::PYTHON_RUNTIME) =>
            {
                CodeEngine::Python
            }
            LANGUAGE_JAVASCRIPT | crate::state::LANGUAGE_PYTHON => {
                let named = runtime.unwrap_or_default();
                return Ok(tool_failed(
                    "executeCode",
                    format!(
                        "the {named} runtime does not run {language}; a runtime names what the \
                         code of that language runs under"
                    ),
                ));
            }
            other => {
                return Ok(ToolOutcome::NothingRan(format!(
                    "This code interpreter session has no {other} engine, so the code was not \
                     run. The engines it embeds run {LANGUAGE_JAVASCRIPT} and {}.",
                    crate::state::LANGUAGE_PYTHON
                )))
            }
        };
        if script.len() > MAX_SCRIPT_BYTES {
            return Ok(ToolOutcome::NothingRan(format!(
                "The submitted script is {} bytes, past the {MAX_SCRIPT_BYTES} this session \
                 compiles, so it was not run.",
                script.len()
            )));
        }

        // The context this run is evaluated on top of and the workspace it reads and
        // writes, copied out under the guard — the context dropped first where the call
        // asked for a clear one. The guard is released before anything is evaluated.
        let (context_scripts, sandbox) = {
            let mut store = state.lock();
            let session = store
                .code_interpreter_session_mut(code_interpreter_identifier, session_id)
                .ok_or_else(|| session_gone(code_interpreter_identifier, session_id))?;
            if arguments.clear_context == Some(true) {
                session.clear_context();
            }
            (
                session.context_scripts_in(&language),
                crate::state::SandboxFilesystem::of_session(session),
            )
        };

        let submitted = script.clone();
        // One of the service's run slots, waited for only as long as [`PERMIT_WAIT`]:
        // past that the caller is throttled rather than left queueing behind other
        // callers' scripts. Held until this run ends, however it ends.
        let permit = ::tokio::time::timeout(PERMIT_WAIT, Arc::clone(&self.runs).acquire_owned())
            .await
            .map_err(|_| {
                throttling(
                    "Too many code executions are running in this code interpreter. Retry the \
                     request.",
                )
            })?
            .map_err(|error| internal(format!("the run slots are gone: {error}")))?;
        // Off the async worker: an engine is synchronous and CPU-bound, and the runtime
        // has to stay responsive while a program runs.
        let (mut run, sandbox) = match engine {
            CodeEngine::JavaScript => {
                // A JavaScript run reaches no filesystem at all: plain QuickJS exposes
                // none, and this module installs none, so the workspace is untouched.
                let run =
                    ::tokio::task::spawn_blocking(move || run_script(&context_scripts, &submitted))
                        .await
                        .map_err(|error| {
                            internal(format!("the script run could not be joined: {error}"))
                        })?;
                (run, None)
            }
            CodeEngine::Python => {
                let filesystem = Arc::new(Mutex::new(sandbox));
                // The engine spawns the run's own thread and bounds the wait for its
                // answer itself, so nothing of the run reaches the async worker.
                let run = crate::state::run_python(&context_scripts, &submitted, &filesystem).await;
                // The workspace the run read and wrote, taken back BY VALUE here — before
                // any state guard is taken, and without ever waiting: `try_unwrap` answers
                // at once, with the workspace when this call is its only holder and with
                // nothing while a run that outlived its bound still holds one. So no lock
                // of a run is ever acquired underneath the guard every other request to
                // this service needs.
                let written = Arc::try_unwrap(filesystem).ok().map(|held| {
                    held.into_inner().unwrap_or_else(PoisonError::into_inner)
                });
                (
                    ScriptRun {
                        stdout: run.stdout,
                        stderr: run.stderr,
                        exit_code: run.exit_code,
                        elapsed: run.elapsed,
                        failed_context: run.failed_context,
                    },
                    written,
                )
            }
        };
        drop(permit);

        {
            let mut store = state.lock();
            let elsewhere =
                store.session_bytes_elsewhere(code_interpreter_identifier, session_id);
            // What the run wrote in the workspace, back into the session: the file tools
            // and the command plane read the same filesystem a run writes. A write the
            // workspace had no room for is said so on the run's own error stream. The
            // workspace is already owned here, so this guarded block takes no other lock.
            if let Some(filesystem) = &sandbox {
                if filesystem.changed() {
                    if let Some(session) =
                        store.code_interpreter_session_mut(code_interpreter_identifier, session_id)
                    {
                        let refused = filesystem.apply_to(session, SystemTime::now(), elsewhere);
                        if refused > 0 {
                            run.stderr.push_str(&workspace_had_no_room(refused));
                        }
                    }
                }
            }
            if !run.failed_context.is_empty() {
                // A context program that no longer runs rebuilds nothing, so it is not
                // part of this session's context any more. Dropped BEFORE this run's own
                // program is kept, because the positions are the ones of the context this
                // run was handed.
                if let Some(session) =
                    store.code_interpreter_session_mut(code_interpreter_identifier, session_id)
                {
                    session.forget_context_scripts(&language, &run.failed_context);
                }
            }
            if run.exit_code == 0 {
                // A program that ran to completion becomes part of what the next run of
                // this session in the same language sees. One that ended on an exception
                // defined nothing a later run can rely on, so it is not kept.
                if let Some(session) =
                    store.code_interpreter_session_mut(code_interpreter_identifier, session_id)
                {
                    session.keep_context_script(&language, script, elsewhere);
                }
            }
        }

        Ok(ran_outcome(run))
    }

    /// `executeCommand`: perform the command line in the session's own workspace and
    /// report the two streams and the status it ended with.
    async fn execute_command(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        arguments: &ToolArguments,
    ) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
        let Some(command) = arguments.command.clone() else {
            return Err(validation(
                "The executeCommand tool requires the command argument.",
            ));
        };
        if let Some(refused) = command_past_the_ceiling("executeCommand", &command) {
            return Ok(refused);
        }
        let sandbox = self.with_session(
            state,
            code_interpreter_identifier,
            session_id,
            crate::state::SandboxFilesystem::of_session,
        )?;
        let (run, sandbox) = self.perform_commands(vec![command], sandbox).await?;
        let Some(run) = run.into_iter().next() else {
            return Err(internal("the command run reported nothing"));
        };
        let refused =
            self.take_workspace_back(state, code_interpreter_identifier, session_id, &sandbox);
        let mut stderr = run.stderr;
        // A file the command wrote that the workspace had no room for is said so on the
        // command's own error stream: the same condition a `writeFiles` call is refused for
        // is never left looking stored.
        if refused > 0 {
            stderr.push_str(&workspace_had_no_room(refused));
        }
        // The time the command's own program asked to take, honoured by answering no
        // sooner than that: the caller observes the span its command waited for. Nothing is
        // held while this call waits — no state guard, no run slot and no thread.
        if !run.slept.is_zero() {
            ::tokio::time::sleep(run.slept).await;
        }
        Ok(ran_outcome(ScriptRun {
            stdout: run.stdout,
            stderr,
            exit_code: run.exit_code,
            elapsed: run.elapsed.saturating_add(run.slept),
            // A command is not evaluated on top of a context, so there is none to report.
            failed_context: Vec::new(),
        }))
    }

    /// `startCommandExecution`: mint a task for the command line and answer with its id
    /// and its status, which is a status nothing has finished yet.
    fn start_command_execution(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        arguments: &ToolArguments,
    ) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
        let Some(command) = arguments.command.clone() else {
            return Err(validation(
                "The startCommandExecution tool requires the command argument.",
            ));
        };
        if let Some(refused) = command_past_the_ceiling("startCommandExecution", &command) {
            return Ok(refused);
        }
        let now = SystemTime::now();
        let mut store = state.lock();
        let seq = store.next_seq();
        let session = store
            .code_interpreter_session_mut(code_interpreter_identifier, session_id)
            .ok_or_else(|| session_gone(code_interpreter_identifier, session_id))?;
        // The tasks one session holds have a ceiling, and a caller chooses how many it
        // submits: a submission the session has no room for is refused in band, exactly as a
        // write a full workspace has no room for is.
        if !session.room_for_a_task(now) {
            return Ok(tool_failed(
                "startCommandExecution",
                format!(
                    "this code interpreter session holds the {} tasks one session may; read \
                     or stop a task you no longer need",
                    crate::state::MAX_SESSION_TASKS
                ),
            ));
        }
        let task = crate::state::CommandTaskRecord::submitted(seq, command, now);
        let reported = task_outcome(&task, now);
        session.submit_task(task);
        Ok(reported)
    }

    /// `getTask`: report the task the call names, as it stands now.
    fn report_task(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        arguments: &ToolArguments,
    ) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
        let task_id = required_task_id("getTask", arguments)?;
        let now = SystemTime::now();
        self.with_session(
            state,
            code_interpreter_identifier,
            session_id,
            |session| match session.tasks.get(&task_id) {
                Some(task) => task_outcome(task, now),
                None => no_such_task("getTask", &task_id),
            },
        )
    }

    /// `stopTask`: cancel the task the call names, so the command it carries is never
    /// performed, and report the status it is in afterwards.
    fn stop_task(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        arguments: &ToolArguments,
    ) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
        let task_id = required_task_id("stopTask", arguments)?;
        let now = SystemTime::now();
        self.with_session_mut(
            state,
            code_interpreter_identifier,
            session_id,
            |session| match session.tasks.get_mut(&task_id) {
                Some(task) => {
                    // A task that has already reached a terminal status is reported as it
                    // stands: there is nothing left to cancel.
                    task.cancel(now);
                    task_outcome(task, now)
                }
                None => no_such_task("stopTask", &task_id),
            },
        )
    }

    /// Perform every task of this session whose command the clock has made due.
    ///
    /// Called at the start of EVERY tool invocation on the session, which is what makes a
    /// submitted task converge: the transition from a non-terminal status to a terminal
    /// one was owed at [`crate::state::CommandTaskRecord::due_at`] whether or not anybody
    /// called, so the first call after that instant is what materialises it. A task a
    /// `stopTask` cancelled first is never performed at all.
    ///
    /// Each task is claimed under the guard, performed with NO guard held, and its answer
    /// stored under the guard again, so one caller's command cannot stall another
    /// caller's request and two callers cannot perform the same task twice.
    async fn converge_due_tasks(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
    ) -> Result<(), InvokeCodeInterpreterErrorModeled> {
        let now = SystemTime::now();
        let (claimed, sandbox) = {
            let mut store = state.lock();
            let Some(session) =
                store.code_interpreter_session_mut(code_interpreter_identifier, session_id)
            else {
                return Ok(());
            };
            let mut claimed: Vec<(String, String)> = Vec::new();
            for task in session.tasks.values_mut() {
                // At most as many as ONE call performs: the rest are performed by the calls
                // after this one, so the work of a call stays proportional to the call
                // rather than to a backlog an earlier caller chose.
                if claimed.len() >= crate::state::MAX_DUE_TASKS_PER_CALL {
                    break;
                }
                if task.due_at_instant(now) {
                    task.performing = true;
                    claimed.push((task.task_id.clone(), task.command.clone()));
                }
            }
            if claimed.is_empty() {
                return Ok(());
            }
            claimed.sort_by(|left, right| left.0.cmp(&right.0));
            (
                claimed,
                crate::state::SandboxFilesystem::of_session(session),
            )
        };

        let commands: Vec<String> = claimed.iter().map(|(_, command)| command.clone()).collect();
        let performed = self.perform_commands(commands, sandbox).await;
        // A claim is RELEASED however the performing ended: a task left claimed would be
        // non-terminal for ever — never due again, and never pruned — so one that could not
        // be performed goes back to being due instead.
        let (runs, sandbox) = match performed {
            Ok(performed) => performed,
            Err(error) => {
                let mut store = state.lock();
                if let Some(session) =
                    store.code_interpreter_session_mut(code_interpreter_identifier, session_id)
                {
                    for (task_id, _) in &claimed {
                        if let Some(task) = session.tasks.get_mut(task_id) {
                            task.performing = false;
                        }
                    }
                }
                return Err(error);
            }
        };

        let stored_at = SystemTime::now();
        let mut store = state.lock();
        let elsewhere = store.session_bytes_elsewhere(code_interpreter_identifier, session_id);
        if let Some(session) =
            store.code_interpreter_session_mut(code_interpreter_identifier, session_id)
        {
            for ((task_id, _), run) in claimed.iter().zip(runs) {
                let Some(task) = session.tasks.get_mut(task_id) else {
                    continue;
                };
                task.performing = false;
                // A task cancelled while its command was being performed keeps the
                // cancelled status it was given.
                if task.canceled_at.is_none() {
                    task.outcome = Some(crate::state::CommandTaskOutcome {
                        stdout: run.stdout,
                        stderr: run.stderr,
                        exit_code: run.exit_code,
                        elapsed: run.elapsed,
                        performed_at: stored_at,
                        // The time the command's own program asked to take: the task is
                        // `working` until it has passed, so a task that waits takes the
                        // time it waited for.
                        finishes_at: stored_at.checked_add(run.slept).unwrap_or(stored_at),
                    });
                }
            }
            let refused = sandbox.apply_to(session, stored_at, elsewhere);
            // A file the task's command wrote that the workspace had no room for is said so
            // on the task's own error stream, rather than left looking stored.
            if refused > 0 {
                for (task_id, _) in &claimed {
                    if let Some(outcome) = session
                        .tasks
                        .get_mut(task_id)
                        .and_then(|task| task.outcome.as_mut())
                    {
                        outcome.stderr.push_str(&workspace_had_no_room(refused));
                    }
                }
            }
        }
        Ok(())
    }

    /// Perform `commands`, in order, over `sandbox` — on a blocking thread, under one of
    /// the service's run slots and with no state guard held.
    async fn perform_commands(
        &self,
        commands: Vec<String>,
        sandbox: crate::state::SandboxFilesystem,
    ) -> Result<
        (
            Vec<crate::state::CommandRun>,
            crate::state::SandboxFilesystem,
        ),
        InvokeCodeInterpreterErrorModeled,
    > {
        let permit = ::tokio::time::timeout(PERMIT_WAIT, Arc::clone(&self.runs).acquire_owned())
            .await
            .map_err(|_| {
                throttling(
                    "Too many code executions are running in this code interpreter. Retry the \
                     request.",
                )
            })?
            .map_err(|error| internal(format!("the run slots are gone: {error}")))?;
        let performed = ::tokio::task::spawn_blocking(move || {
            let mut sandbox = sandbox;
            let runs: Vec<crate::state::CommandRun> = commands
                .iter()
                .map(|command| crate::state::run_command(command, &mut sandbox))
                .collect();
            (runs, sandbox)
        })
        .await
        .map_err(|error| internal(format!("the command run could not be joined: {error}")))?;
        drop(permit);
        Ok(performed)
    }

    /// Put what a run or a command wrote in `sandbox` back into the session's workspace,
    /// answering how many of those writes the workspace had no room for.
    fn take_workspace_back(
        &self,
        state: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
        >,
        code_interpreter_identifier: &str,
        session_id: &str,
        sandbox: &crate::state::SandboxFilesystem,
    ) -> usize {
        if !sandbox.changed() {
            return 0;
        }
        let mut store = state.lock();
        let elsewhere = store.session_bytes_elsewhere(code_interpreter_identifier, session_id);
        match store.code_interpreter_session_mut(code_interpreter_identifier, session_id) {
            Some(session) => sandbox.apply_to(session, SystemTime::now(), elsewhere),
            None => 0,
        }
    }
}

/// What a run or a command is told when the workspace had no room for the files it wrote:
/// the same condition a `writeFiles` call is refused for, on the stream the run reports its
/// own failures on.
fn workspace_had_no_room(refused: usize) -> String {
    format!(
        "The session workspace had no room for {refused} of the files this run wrote, so they \
         were not stored.\n"
    )
}

/// Which engine a submitted program runs in, as its `language` and `runtime` name it.
enum CodeEngine {
    /// The QuickJS engine embedded in this process.
    JavaScript,
    /// The contained Python interpreter this crate embeds
    /// ([`crate::state::run_python`]).
    Python,
}

/// The result for a program or a command this emulator RAN: the text a client reads is
/// the output it produced, and the structured half is the run's own report.
fn ran_outcome(run: ScriptRun) -> ToolOutcome {
    let text = if run.stdout.is_empty() && !run.stderr.is_empty() {
        run.stderr.clone()
    } else {
        run.stdout.clone()
    };
    ToolOutcome::Ran {
        content: vec![text_block(text)],
        stdout: run.stdout,
        stderr: run.stderr,
        exit_code: run.exit_code,
        elapsed: run.elapsed,
    }
}

/// The in-band failure for a command line longer than one call carries, or `None` for one
/// inside that ceiling ([`crate::state::MAX_COMMAND_BYTES`]).
///
/// Checked before anything is performed OR stored, so neither the shell nor a submitted
/// task's record is ever handed a line past it.
fn command_past_the_ceiling(tool: &str, command: &str) -> Option<ToolOutcome> {
    (command.len() > crate::state::MAX_COMMAND_BYTES).then(|| {
        tool_failed(
            tool,
            format!(
                "the command is {} bytes, past the {} this session performs",
                command.len(),
                crate::state::MAX_COMMAND_BYTES
            ),
        )
    })
}

/// The `taskId` a task tool cannot work without.
fn required_task_id(
    tool: &str,
    arguments: &ToolArguments,
) -> Result<String, InvokeCodeInterpreterErrorModeled> {
    arguments
        .task_id
        .clone()
        .filter(|task_id| !task_id.is_empty())
        .ok_or_else(|| validation(format!("The {tool} tool requires the taskId argument.")))
}

/// How a task is reported, whichever tool reports it: its id and its status always, and
/// the two streams, the exit status and the duration once its work is over \u2014 not before,
/// because a task that is still working has produced no status to report.
fn task_outcome(task: &crate::state::CommandTaskRecord, now: SystemTime) -> ToolOutcome {
    let status = task.status_as_of(now);
    let finished = matches!(
        status,
        crate::state::TASK_STATUS_COMPLETED | crate::state::TASK_STATUS_FAILED
    );
    let run = task.outcome.as_ref().filter(|_| finished).map(|outcome| TaskRun {
        stdout: outcome.stdout.clone(),
        stderr: outcome.stderr.clone(),
        exit_code: outcome.exit_code,
        elapsed: task.reported_duration().unwrap_or(outcome.elapsed),
    });
    let text = match &run {
        Some(run) if !run.stdout.is_empty() => run.stdout.clone(),
        Some(run) if !run.stderr.is_empty() => run.stderr.clone(),
        _ => format!("Task {} is {status}.", task.task_id),
    };
    ToolOutcome::Task {
        content: vec![text_block(text)],
        task_id: task.task_id.clone(),
        task_status: status,
        run,
    }
}

/// The in-band failure for a task tool whose `taskId` names no task of this session.
fn no_such_task(tool: &str, task_id: &str) -> ToolOutcome {
    tool_failed(
        tool,
        format!("this code interpreter session holds no task {task_id}"),
    )
}

/// What one tool invocation did, before it is worded as a `CodeInterpreterResult`.
///
/// Which members the result carries follows from the KIND of tool:
///
/// * `structuredContent` reports a RUN — `stdout`, `stderr`, `exitCode`,
///   `executionTime`, `taskId`, `taskStatus` are its only members — so it belongs to a
///   tool that ran code, performed a command or reports a task, and a tool that reads
///   or writes the workspace, or whose work nothing performed, has nothing to put in
///   it;
/// * `isError` is the flag that distinguishes a result that "represents an error" from
///   one carrying "successful execution results", so it is reported on every result —
///   `true` for a tool that could not do what was asked, `false` for one that did.
enum ToolOutcome {
    /// A workspace tool that did what was asked: the blocks a client reads, and
    /// nothing else.
    Content(Vec<ContentBlock>),
    /// A tool that ran and could not do what was asked — a path that does not address
    /// the session workspace, a path the workspace does not hold, a workspace with no
    /// room left, a task id that names no task. Reported in band, as the lead-in the
    /// tool plane words every tool-level failure with ([`Self::into_result`]).
    Failed {
        /// The tool that could not do what was asked, which the lead-in names.
        tool: String,
        /// Why it could not, worded for the caller that sent the arguments.
        reason: String,
    },
    /// A program or a command this emulator RAN: the blocks a client reads, plus the
    /// report of the run itself (`0` for work that ran to completion) and how long THAT
    /// run took — not how long the call around it took.
    Ran {
        content: Vec<ContentBlock>,
        stdout: String,
        stderr: String,
        exit_code: i32,
        elapsed: Duration,
    },
    /// A TASK of the asynchronous command plane, as the tool that submitted, read or
    /// cancelled it reports it: its id and its status, plus the run once its command has
    /// been performed.
    Task {
        content: Vec<ContentBlock>,
        task_id: String,
        task_status: &'static str,
        run: Option<TaskRun>,
    },
    /// Work this emulator embeds no engine for, so nothing performed it.
    NothingRan(String),
}

/// What performing one task's command came to, as a task report carries it.
struct TaskRun {
    stdout: String,
    stderr: String,
    exit_code: i32,
    elapsed: Duration,
}

/// The in-band failure of one tool: the lead-in AWS words a tool-level failure with,
/// the tool's name, and why it could not do what was asked.
fn tool_failed(tool: &str, reason: impl Into<String>) -> ToolOutcome {
    ToolOutcome::Failed {
        tool: tool.to_string(),
        reason: reason.into(),
    }
}

impl ToolOutcome {
    /// The `result` event's own shape: the blocks, the structured report where the tool
    /// performed a run, and `isError` — always, because the flag is what tells a client
    /// reading a 200 whether the tool succeeded.
    fn into_result(self) -> CodeInterpreterResult {
        match self {
            Self::Content(content) => CodeInterpreterResult {
                content,
                structured_content: None,
                is_error: Some(false),
            },
            // A tool-level failure is reported on the result rather than as an API
            // error, with the lead-in the tool plane words every one of them with: the
            // fixed `Error executing tool `, the tool's own name, and the reason. No
            // file content and no resource link accompanies it — the tool did not do
            // what was asked, so there is nothing of the workspace to carry.
            Self::Failed { tool, reason } => CodeInterpreterResult {
                content: vec![text_block(format!("Error executing tool {tool}: {reason}"))],
                structured_content: None,
                is_error: Some(true),
            },
            Self::Ran {
                content,
                stdout,
                stderr,
                exit_code,
                elapsed,
            } => CodeInterpreterResult {
                content,
                structured_content: Some(ToolResultStructuredContent {
                    task_id: None,
                    task_status: None,
                    stdout: Some(stdout),
                    stderr: Some(stderr),
                    exit_code: Some(exit_code),
                    // Milliseconds, as a `Double`: the member is documented as "the
                    // execution time of the tool operation in milliseconds" (source:
                    // smithy_query query="shape"
                    // shape="ToolResultStructuredContent"), and this is the engine's own
                    // time on this run.
                    execution_time: Some(elapsed.as_secs_f64() * MILLISECONDS_PER_SECOND),
                }),
                is_error: Some(exit_code != 0),
            },
            // A task report: its id and its status always, and the run's own report once
            // the command has been performed. A task whose command FAILED is an error
            // result, on the same rule a synchronous run follows; one that was cancelled
            // is not — the caller asked for that.
            Self::Task {
                content,
                task_id,
                task_status,
                run,
            } => CodeInterpreterResult {
                content,
                structured_content: Some(ToolResultStructuredContent {
                    task_id: Some(task_id),
                    task_status: Some(task_status.to_string()),
                    stdout: run.as_ref().map(|run| run.stdout.clone()),
                    stderr: run.as_ref().map(|run| run.stderr.clone()),
                    exit_code: run.as_ref().map(|run| run.exit_code),
                    execution_time: run
                        .as_ref()
                        .map(|run| run.elapsed.as_secs_f64() * MILLISECONDS_PER_SECOND),
                }),
                is_error: Some(run.as_ref().is_some_and(|run| run.exit_code != 0)),
            },
            // Nothing performed this work, so there is no run to report: no output, no
            // exit status, no duration — and no structured report at all, which would
            // say a run happened and carry a diagnostic as though a tool had written it
            // to a stream.
            Self::NothingRan(reason) => CodeInterpreterResult {
                content: vec![text_block(reason)],
                structured_content: None,
                is_error: Some(true),
            },
        }
    }
}

/// The 404 for a session that is no longer held — a stop that landed between this
/// call's own steps.
fn session_gone(
    code_interpreter_identifier: &str,
    session_id: &str,
) -> InvokeCodeInterpreterErrorModeled {
    InvokeCodeInterpreterErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(format!(
            "session {session_id} not found for code interpreter {code_interpreter_identifier}"
        )),
    })
}

/// One `text` content block.
fn text_block(text: impl Into<String>) -> ContentBlock {
    ContentBlock {
        r#type: BLOCK_TEXT.to_string(),
        text: Some(text.into()),
        ..Default::default()
    }
}

/// `writeFiles`: put every block's bytes in the session's workspace.
///
/// All or nothing: the paths are normalized and the bytes taken first, a block that
/// names a path an earlier block in the same call named is that path's last write, and
/// the WHOLE batch is then checked against the workspace ceilings before anything is
/// stored — so a call the workspace has no room for leaves it exactly as it was.
fn write_files(
    session: &mut crate::state::CodeInterpreterSessionRecord,
    arguments: &ToolArguments,
    now: SystemTime,
    bytes_elsewhere: usize,
) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
    let blocks = arguments.content.as_deref().unwrap_or_default();
    if blocks.is_empty() {
        return Err(validation(
            "The writeFiles tool requires the content argument to carry at least one file.",
        ));
    }
    if blocks.len() > MAX_TOOL_PATHS {
        return Err(too_many_paths("writeFiles", blocks.len()));
    }
    let mut pending: Vec<(String, Vec<u8>)> = Vec::with_capacity(blocks.len());
    for block in blocks {
        let key = match workspace_key("writeFiles", &block.path) {
            Ok(key) => key,
            Err(refused) => return *refused,
        };
        let bytes = match (block.text.as_ref(), block.blob.as_ref()) {
            (Some(text), None) => text.as_bytes().to_vec(),
            (None, Some(blob)) => blob.clone(),
            (None, None) => Vec::new(),
            (Some(_), Some(_)) => {
                return Err(validation(format!(
                    "The writeFiles content block for {key} carries both text and blob; it must \
                     carry one."
                )))
            }
        };
        // One entry per path, so the room this batch needs is the room its last write
        // of each path needs.
        match pending.iter_mut().find(|(held, _)| *held == key) {
            Some(held) => held.1 = bytes,
            None => pending.push((key, bytes)),
        }
    }

    // The ceilings are checked over the whole batch, where the store owns the
    // arithmetic, so no part of a batch that does not fit is stored.
    let sizes: Vec<(String, usize)> = pending
        .iter()
        .map(|(key, bytes)| (key.clone(), bytes.len()))
        .collect();
    if let Err(full) = session.room_for(&sizes, bytes_elsewhere) {
        return Ok(workspace_full("writeFiles", full, Some(pending.len())));
    }

    let mut written = String::new();
    for (key, bytes) in pending {
        let size = bytes.len();
        // The batch already has room, so this cannot report the workspace full; the
        // store checks each write anyway, and its answer is reported rather than
        // dropped.
        if let Err(full) = session.write_file(&key, bytes, now, bytes_elsewhere) {
            return Ok(workspace_full("writeFiles", full, None));
        }
        written.push_str(&format!("Wrote {key} ({size} bytes)\n"));
    }
    Ok(ToolOutcome::Content(vec![text_block(written)]))
}

/// `readFiles`: the contents of every path named, as one `resource` block each.
fn read_files(
    session: &crate::state::CodeInterpreterSessionRecord,
    arguments: &ToolArguments,
) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
    let keys = match workspace_keys(arguments, "readFiles") {
        Ok(keys) => keys,
        Err(refused) => return *refused,
    };
    let missing: Vec<&String> = keys
        .iter()
        .filter(|key| !session.files.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        return Ok(not_in_workspace("readFiles", &missing));
    }
    let mut content = Vec::with_capacity(keys.len());
    // What this result has already materialised, so the response stays proportional to
    // the call rather than to the count times the size a caller chose.
    let mut emitted = 0usize;
    for key in &keys {
        let Some(file) = session.files.get(key) else {
            continue;
        };
        emitted = emitted.saturating_add(file.bytes.len());
        if emitted > MAX_READ_RESULT_BYTES {
            return Ok(tool_failed(
                "readFiles",
                format!(
                    "the files named hold more than the {MAX_READ_RESULT_BYTES} bytes one \
                     readFiles result carries, so nothing was read; read fewer files per call"
                ),
            ));
        }
        let media_type = media_type_of(key);
        // Text where the stored bytes are text, bytes where they are not: the file's
        // own content decides, not its name.
        let resource = match String::from_utf8(file.bytes.clone()) {
            Ok(text) => ResourceContent {
                r#type: RESOURCE_TEXT.to_string(),
                uri: Some(workspace_uri(key)),
                mime_type: Some(media_type),
                text: Some(text),
                blob: None,
            },
            Err(_) => ResourceContent {
                r#type: RESOURCE_BLOB.to_string(),
                uri: Some(workspace_uri(key)),
                mime_type: Some(media_type),
                text: None,
                blob: Some(file.bytes.clone()),
            },
        };
        content.push(ContentBlock {
            r#type: BLOCK_RESOURCE.to_string(),
            resource: Some(resource),
            ..Default::default()
        });
    }
    Ok(ToolOutcome::Content(content))
}

/// `listFiles`: every workspace entry directly under the directory named, as one
/// `resource_link` block each.
fn list_files(
    session: &crate::state::CodeInterpreterSessionRecord,
    arguments: &ToolArguments,
) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
    let named = arguments.directory_path.as_deref().unwrap_or_default();
    let directory = match crate::state::normalize_session_directory(named) {
        Ok(directory) => directory,
        Err(rejection) => {
            return Ok(tool_failed(
                "listFiles",
                path_rejection_reason(named, rejection),
            ))
        }
    };
    let content: Vec<ContentBlock> = session
        .list_directory(&directory)
        .into_iter()
        .map(|(key, size)| ContentBlock {
            r#type: BLOCK_RESOURCE_LINK.to_string(),
            uri: Some(workspace_uri(&key)),
            name: Some(key.rsplit('/').next().unwrap_or(&key).to_string()),
            mime_type: Some(match size {
                Some(_) => media_type_of(&key),
                None => DIRECTORY_MEDIA_TYPE.to_string(),
            }),
            size: size.and_then(|size| i64::try_from(size).ok()),
            ..Default::default()
        })
        .collect();
    Ok(ToolOutcome::Content(content))
}

/// `removeFiles`: drop every path named from the session's workspace.
fn remove_files(
    session: &mut crate::state::CodeInterpreterSessionRecord,
    arguments: &ToolArguments,
) -> Result<ToolOutcome, InvokeCodeInterpreterErrorModeled> {
    let keys = match workspace_keys(arguments, "removeFiles") {
        Ok(keys) => keys,
        Err(refused) => return *refused,
    };
    let missing: Vec<&String> = keys
        .iter()
        .filter(|key| !session.files.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        return Ok(not_in_workspace("removeFiles", &missing));
    }
    let mut removed = String::new();
    for key in &keys {
        session.files.remove(key);
        removed.push_str(&format!("Removed {key}\n"));
    }
    Ok(ToolOutcome::Content(vec![text_block(removed)]))
}

/// The in-band failure for a tool whose paths address nothing in the workspace: the tool
/// ran and could not do what was asked.
fn not_in_workspace(tool: &str, missing: &[&String]) -> ToolOutcome {
    let listed = missing
        .iter()
        .map(|key| key.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    tool_failed(
        tool,
        format!("the session workspace holds no such file: {listed}"),
    )
}

/// The in-band failure for a workspace with no room left for what a tool was asked to
/// write, naming the ceiling it met.
///
/// `batch` is how many files the call named, for the write that is refused BEFORE any of
/// them is stored.
fn workspace_full(
    tool: &str,
    full: crate::state::WorkspaceFull,
    batch: Option<usize>,
) -> ToolOutcome {
    let refused = match batch {
        Some(files) => format!("none of the {files} files this call names was written"),
        None => "the file was not written".to_string(),
    };
    tool_failed(
        tool,
        match full {
            crate::state::WorkspaceFull::Files(limit) => {
                format!("the session workspace holds at most {limit} files, so {refused}")
            }
            crate::state::WorkspaceFull::Bytes(limit) => {
                format!("the session workspace holds at most {limit} bytes, so {refused}")
            }
            crate::state::WorkspaceFull::StoreBytes(limit) => format!(
                "the code interpreter sessions of this emulator together hold at most {limit} \
                 bytes, so {refused}; stop a session you no longer need, or remove files from it"
            ),
        },
    )
}

/// The workspace key one tool `path` addresses, or how the tool reports that it
/// addresses none.
///
/// A path that does not address the session workspace is a failure of the invoked TOOL,
/// not of the request: it is reported IN BAND, on the result, which is why the `Err` side
/// here carries a whole answer — the in-band `Ok(Failed)` for a path, or the modelled
/// error for a request the tool cannot be invoked with at all — for the caller to return
/// as it stands.
fn workspace_key(tool: &str, path: &str) -> Result<String, Box<ToolAnswer>> {
    crate::state::normalize_session_path(path).map_err(|rejection| {
        Box::new(Ok(tool_failed(tool, path_rejection_reason(path, rejection))))
    })
}

/// What one of the file tools answers: the outcome it computed, or the modelled error for
/// a request it could not be invoked with.
///
/// It is BOXED where it travels as the `Err` side of a path lookup ([`workspace_key`]), so
/// the lookup's own result stays small: a whole answer is the larger of the two sides, and a
/// function that returns one by value would carry that on every call.
type ToolAnswer = Result<ToolOutcome, InvokeCodeInterpreterErrorModeled>;

/// The workspace keys a tool's `paths` (or its single `path`) addresses, each named
/// once: repeating a path addresses the same file, and de-duplicating is what keeps one
/// repeated name from multiplying the work and the response.
fn workspace_keys(arguments: &ToolArguments, tool: &str) -> Result<Vec<String>, Box<ToolAnswer>> {
    let named: Vec<String> = match (arguments.paths.as_deref(), arguments.path.as_deref()) {
        (Some(paths), _) if !paths.is_empty() => paths.to_vec(),
        (_, Some(path)) => vec![path.to_string()],
        _ => {
            return Err(Box::new(Err(validation(format!(
                "The {tool} tool requires the paths argument."
            )))))
        }
    };
    if named.len() > MAX_TOOL_PATHS {
        return Err(Box::new(Err(too_many_paths(tool, named.len()))));
    }
    let mut keys: Vec<String> = Vec::with_capacity(named.len());
    for path in &named {
        let key = workspace_key(tool, path)?;
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    Ok(keys)
}

/// The 400 for a tool call that addresses more files than one call may.
fn too_many_paths(tool: &str, named: usize) -> InvokeCodeInterpreterErrorModeled {
    validation(format!(
        "The {tool} tool was given {named} paths; at most {MAX_TOOL_PATHS} are accepted in one \
         call."
    ))
}

/// Why a path addresses nothing in the session workspace, worded for the caller that sent
/// it, as the reason half of the in-band failure ([`tool_failed`]).
///
/// A path that is absolute and one that walks out through a parent segment are the same
/// condition — the path does not stay inside the session's own workspace — so both are
/// reported as the traversal they are, naming the path the request carried.
fn path_rejection_reason(path: &str, rejection: crate::state::PathRejection) -> String {
    match rejection {
        crate::state::PathRejection::Empty => {
            "a tool path must name a file in the session workspace".to_string()
        }
        crate::state::PathRejection::Absolute => format!(
            "path traversal is not allowed: the path {path} is absolute, and a tool path \
             addresses the session workspace and is relative to it"
        ),
        crate::state::PathRejection::Escapes => format!(
            "path traversal is not allowed: the path {path} leaves the session workspace \
             through a parent segment"
        ),
        crate::state::PathRejection::TooLong => format!(
            "the tool path {path} is longer than the {} bytes or {} segments a session \
             workspace path has",
            crate::state::MAX_SESSION_PATH_BYTES,
            crate::state::MAX_SESSION_PATH_SEGMENTS
        ),
    }
}

/// The URI a workspace entry is named by: the workspace is the session's own, so the
/// key IS the whole path.
fn workspace_uri(key: &str) -> String {
    format!("file:///{key}")
}

/// The media type a workspace entry's name implies, which is the tree's ONE reading of
/// what a file extension names (`service_common::web_media_type`), so this plane cannot
/// answer an extension differently from the ones that serve deployed files.
fn media_type_of(key: &str) -> String {
    ::service_common::web_media_type::media_type_of_path(key).to_string()
}

impl<Eng: Send + Sync + 'static> service_common::event_stream::EventStreamOperation
    for InvokeCodeInterpreter<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let response = match self.respond(protocol, req).await {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            };
            // One access record per request, so a refused invocation is diagnosable.
            // No request payload and no credential reaches it.
            tracing::debug!(
                operation = "InvokeCodeInterpreter",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}

// =============================================================================
// The JavaScript engine: one run, bounded and contained
// =============================================================================

/// What one script run produced.
struct ScriptRun {
    stdout: String,
    stderr: String,
    /// `0` when the script ran to completion, `1` when it ended on an uncaught
    /// exception or hit one of this module's bounds — the status a script host reports
    /// for each.
    exit_code: i32,
    /// How long the engine spent on this run, which is what the result reports as its
    /// `executionTime`: measured around the evaluation only, so no permit wait or lock
    /// acquisition is counted as the script's own time.
    elapsed: Duration,
    /// Which of the session's CONTEXT scripts no longer run, by their position in the
    /// context this run was handed — see [`crate::state::PythonRun::failed_context`].
    failed_context: Vec<usize>,
}

/// What a run wrote to its output streams, as the host collected it.
#[derive(Debug, Default)]
struct Capture {
    stdout: String,
    stderr: String,
    /// Output is collected only while the script the CALLER submitted runs: the
    /// context scripts before it already reported their output to the calls that ran
    /// them, and reporting it twice would be output no caller's script produced.
    recording: bool,
}

impl Capture {
    /// Append `text` to one stream, stopping at [`MAX_CAPTURED_STREAM_BYTES`].
    fn write(&mut self, to_stderr: bool, text: &str) {
        if !self.recording {
            return;
        }
        let stream = if to_stderr {
            &mut self.stderr
        } else {
            &mut self.stdout
        };
        let room = MAX_CAPTURED_STREAM_BYTES.saturating_sub(stream.len());
        if room == 0 {
            return;
        }
        // On a character boundary, so a capped stream is still a string.
        let mut end = text.len().min(room);
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        stream.push_str(&text[..end]);
    }
}

/// The guard over the capture, recovering a poisoned lock rather than panicking with
/// it: the capture is this run's own, so whatever a panic left in it is still the only
/// output there is.
fn captured(capture: &Arc<Mutex<Capture>>) -> ::std::sync::MutexGuard<'_, Capture> {
    capture.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The `console` a script writes to. Defined in JavaScript over the two host functions
/// installed beside it, so the formatting a caller sees is the engine's own.
const CONSOLE_PRELUDE: &str = r#"
globalThis.console = (function () {
    var text = function (value) {
        if (typeof value === "string") { return value; }
        if (value instanceof Error) { return value.stack ? String(value.stack) : String(value); }
        try {
            var json = JSON.stringify(value);
            return json === undefined ? String(value) : json;
        } catch (error) {
            return String(value);
        }
    };
    var line = function (args) {
        return Array.prototype.map.call(args, text).join(" ") + "\n";
    };
    var out = function () { __hostStdout(line(arguments)); };
    var err = function () { __hostStderr(line(arguments)); };
    return { log: out, info: out, debug: out, dir: out, warn: err, error: err, trace: err };
})();
"#;

/// Run `script` in a fresh engine, on top of `context_scripts`.
///
/// Called on a blocking thread with no state guard held. Every bound this module
/// declares is set on the engine before anything is evaluated, so a script cannot
/// outrun them.
fn run_script(context_scripts: &[String], script: &str) -> ScriptRun {
    let started = Instant::now();
    let capture = Arc::new(Mutex::new(Capture::default()));
    let failed_context = Arc::new(Mutex::new(Vec::<usize>::new()));
    let outcome = evaluate(&capture, &failed_context, context_scripts, script);
    let elapsed = started.elapsed();
    let captured = captured(&capture);
    let mut stdout = captured.stdout.clone();
    let mut stderr = captured.stderr.clone();
    drop(captured);
    let failed_context = failed_context
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    match outcome {
        Ok(()) => ScriptRun {
            stdout,
            stderr,
            exit_code: 0,
            elapsed,
            failed_context,
        },
        Err(failure) => {
            // The engine stops a script that outran the wall clock by raising inside
            // it; say so as what it is rather than passing the engine's interrupt on.
            let reason = if elapsed >= RUN_WALL_CLOCK {
                format!(
                    "The script was stopped after {} seconds.",
                    RUN_WALL_CLOCK.as_secs()
                )
            } else {
                failure
            };
            if !stderr.is_empty() && !stderr.ends_with('\n') {
                stderr.push('\n');
            }
            stderr.push_str(&reason);
            stderr.push('\n');
            if !stdout.is_empty() && !stdout.ends_with('\n') {
                stdout.push('\n');
            }
            ScriptRun {
                stdout,
                stderr,
                exit_code: 1,
                elapsed,
                failed_context,
            }
        }
    }
}

/// Build the engine, install the console, replay the session's context and evaluate
/// the submitted script. `Err` carries the engine's own account of what went wrong.
fn evaluate(
    capture: &Arc<Mutex<Capture>>,
    failed_context: &Arc<Mutex<Vec<usize>>>,
    context_scripts: &[String],
    script: &str,
) -> Result<(), String> {
    let runtime = Runtime::new().map_err(|error| format!("the engine did not start: {error}"))?;
    runtime.set_memory_limit(RUN_MEMORY_BYTES);
    runtime.set_max_stack_size(RUN_STACK_BYTES);
    let deadline = Instant::now() + RUN_WALL_CLOCK;
    // The only thing that can stop a script with no exit: a timeout cannot preempt a
    // loop running on this thread, but the engine asks this handler as it works.
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context =
        Context::full(&runtime).map_err(|error| format!("the engine did not start: {error}"))?;

    context.with(|ctx| {
        install_console(&ctx, capture)?;
        // The context this session already built, re-evaluated with its output not
        // collected, so the variables and functions earlier runs defined are in scope. A
        // context script that no longer runs is not this run's failure: it is reported for
        // the caller to drop from the session's context, and the rest still run.
        for (position, earlier) in context_scripts.iter().enumerate() {
            if run_one(&ctx, earlier).is_err() {
                failed_context
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(position);
            }
        }
        captured(capture).recording = true;
        run_one(&ctx, script)
    })
}

/// Evaluate one script in `ctx`, turning an uncaught exception into the text a caller
/// sees on `stderr`.
fn run_one(ctx: &Ctx<'_>, script: &str) -> Result<(), String> {
    ctx.eval::<Value<'_>, _>(script)
        .catch(ctx)
        .map(|_| ())
        .map_err(|caught| caught.to_string())
}

/// Install the two host functions the console writes through, then the console itself.
///
/// These are the ONLY host functions a script can reach: there is no module loader, and
/// plain QuickJS exposes no filesystem, process, environment or network API.
fn install_console(ctx: &Ctx<'_>, capture: &Arc<Mutex<Capture>>) -> Result<(), String> {
    let to_stdout = Arc::clone(capture);
    ctx.globals()
        .set(
            "__hostStdout",
            Func::from(move |text: String| captured(&to_stdout).write(false, &text)),
        )
        .map_err(|error| format!("the console could not be installed: {error}"))?;
    let to_stderr = Arc::clone(capture);
    ctx.globals()
        .set(
            "__hostStderr",
            Func::from(move |text: String| captured(&to_stderr).write(true, &text)),
        )
        .map_err(|error| format!("the console could not be installed: {error}"))?;
    run_one(ctx, CONSOLE_PRELUDE)
}
