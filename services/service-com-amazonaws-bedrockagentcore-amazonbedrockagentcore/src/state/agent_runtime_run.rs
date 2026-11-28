// SPDX-License-Identifier: CC0-1.0

//! The INVOCATION of a deployed agent: the agent's own code, RUN in the one contained
//! interpreter this workspace embeds, and what it answered.
//!
//! An agent runtime deployed from a `codeConfiguration` artifact runs the caller's own
//! Python ("the runtime environment for executing the agent code", `smithy_query
//! shape=CodeConfiguration`), and on AWS the invocation is delivered to the server that code
//! stands up inside the container the service builds for it: `POST /invocations`. What is
//! delivered HERE is the same thing — the request's payload reaches the entry point the
//! deployed code serves, and the body the invocation answers with is what that code produced.
//! Nothing about an answer is composed by this emulator: a body it wrote itself, or the
//! request handed back, would not be the deployed code's answer.
//!
//! The deployed code serves that invocation in one of the three ways the contract admits, and
//! an invocation is delivered by whichever of them the deployment uses:
//!
//! * the HTTP REQUEST HANDLER it serves the route with — the container contract itself. The
//!   handler is driven over an in-memory connection ([`_Connection`] in
//!   [`CONTRACT_SOURCE`]): it reads the request this emulator wrote and writes the response
//!   that becomes the invocation's status, media type and body;
//! * the entry point it registered with the AgentCore APP it imports ([`APP_SOURCE`]), which
//!   is the package that serves that same route for it;
//! * the callable the artifact's own `entryPoint` names, which is what that member is for:
//!   "the entry point for the code execution, specifying the function or method that should
//!   be invoked when the code runs" (`smithy_query shape=CodeConfiguration`).
//!
//! A deployment that serves none of the three is reported as work nothing ran, not answered
//! with a body: there is no entry point to deliver the invocation to.
//!
//! # The engine is NOT this module's
//!
//! No second execution substrate is introduced here. The interpreter is
//! [`::service_com_amazonaws_lambda_awsgirapiservice::state::execution::python`] — the one
//! contained RustPython this workspace already holds for running caller-supplied Python,
//! driven exactly as this crate's code interpreter drives it
//! ([`super::code_interpreter_python`]). Everything that CONTAINS and BOUNDS a run therefore
//! lives in that one module and is maintained once: its native modules are an allow-list
//! with no socket, subprocess, memory-map, native-library or thread module in it, its
//! `posix` is that crate's own with every host-reaching call refused, every way of opening a
//! HOST file is taken away before any guest code runs, and the deadline, traced-step,
//! memory, recursion and output bounds are enforced in host code the deployed agent cannot
//! reach — including the latch that makes a stopped run's answer that stop whatever the
//! agent catches.
//!
//! What this module adds is the AgentCore RUNTIME CONTRACT, and nothing else:
//!
//! * the CONTRACT module ([`CONTRACT_SOURCE`]) — ordinary Python, handed to the interpreter
//!   as the module the run resolves: it imports the module the artifact's `entryPoint` names,
//!   delivers the invocation the way that deployment serves it, and answers with what the
//!   code produced;
//! * the AgentCore APP a deployed agent registers its entry point with ([`APP_SOURCE`]). On
//!   AWS that package runs an HTTP server inside the container; here the HOST delivers the
//!   invocation, so the app records what was registered and starts nothing;
//! * the host-side bounds around the run: it happens on a thread of its own with the stack a
//!   run is given, and this module waits for its answer no longer than the host time one run
//!   may take and a grace on top of it — past that the run has been given up on and the call
//!   is answered rather than left waiting. The run's own span is read against a clock the RUN
//!   drives, so which bound stopped an invocation is what the agent did and not how fast this
//!   host happened to be running.
//!
//! The LISTENER the deployed code would open is the host, so the contract module puts its own
//! server class in place of the ones a deployment stands its server up with: that is what
//! makes the invocation deliverable, and it is NOT what keeps the agent off this machine —
//! the engine registers no socket module at all, so no guest code of any shape opens a socket
//! here.
//!
//! Both supplied modules are put on the import path AFTER the artifact's own files, so a
//! deployment that ships a copy of the app package resolves the one this emulator serves the
//! invocation through rather than a vendored copy that would try to open a socket.

use std::collections::BTreeMap;
use std::time::Duration;

use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::python::{
    self, Budget, Completion, Stop,
};
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    ENVIRONMENT_BUILD_SPAN, INIT_PHASE_LIMIT,
};

/// The module the runtime contract is handed to the interpreter as, and the entry point it
/// resolves in it.
const CONTRACT_MODULE: &str = "_agentcore_runtime_contract.py";
/// See [`CONTRACT_MODULE`].
const CONTRACT_HANDLER: &str = "_agentcore_runtime_contract.invoke";

/// The package a deployed agent imports to serve its invocations, as the import path holds
/// it: the top-level module, and the `runtime` module the app class is read off.
const APP_PACKAGE: &str = "bedrock_agentcore/__init__.py";
/// See [`APP_PACKAGE`].
const APP_RUNTIME_MODULE: &str = "bedrock_agentcore/runtime/__init__.py";

/// The stack the thread one invocation happens on is given.
///
/// CHOSEN: the interpreter is recursive over the agent's own syntax, so a run needs more
/// stack than a request thread carries; it is the same order as the stack this emulator
/// gives any other run of caller-supplied code.
const RUN_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How long one invocation of a deployed agent may run for, in the RUN'S OWN time.
///
/// An EMULATOR bound, not an AWS one: the agent runs in this process, so a loop with no exit
/// would otherwise hold a thread for ever. Long enough for any answer an agent computes in
/// process, and far short of what AWS allows a session, because nothing here waits on a
/// model or a network.
///
/// It is a span of the RUN's own time rather than of this host's: it is read against a clock
/// the run itself drives ([`AGENT_RUN_CLOCK_TICK`]) and spent by the agent's own doings — one
/// tick per traced event it delivers, and the waits it asks for — and by nothing of this
/// host's speed. So what an invocation answers is what the deployed code produced on an idle
/// host and on a loaded one alike: the interpreter one invocation runs in is built for it and
/// the deployed code's own imports are read in it, work this emulator may need several times
/// as long over when the host is busy, and none of it is the agent's doing. What a run may
/// take of THIS host is bounded beside it ([`AGENT_RUN_HOST_CEILING`]).
pub const AGENT_RUN_WALL_CLOCK: Duration = Duration::from_secs(10);

/// How many traced events ONE invocation may spend, which is the step budget it is given
/// ([`agent_run_budget`]).
///
/// Named here because the tick one reading of the invocation's clock moves
/// ([`AGENT_RUN_CLOCK_TICK`]) is sized against it: the two together decide which bound answers
/// a run that spends everything it is given.
const AGENT_RUN_STEPS: u32 = 50_000_000;

/// How many times the engine reads ONE invocation's clock per traced event the run delivers.
///
/// MEASURED, not chosen: the engine reads the clock to compare the run against its deadline
/// and reads it again for the span the run has spent of that deadline, so one traced event
/// moves a clock the run drives by this many ticks. The tick below has to be sized against
/// it: sized as though an event moved the clock once, the traced events a run is allowed
/// would spend the whole of the invocation's own span, and a run that spent the work it is
/// given would be reported as having run out of TIME rather than on the work it did.
const CLOCK_READINGS_PER_TRACED_EVENT: u32 = 2;

/// What share of the invocation's own span the WHOLE step budget may spend: one over this.
///
/// CHOSEN: room beside the readings the run's own events take — the host's own watch over the
/// run reads the same clock, and so does every seam the run makes a blocking call through —
/// so no reading this emulator took of its own accord spends the span the agent is given.
const STEP_BUDGET_SHARE_OF_THE_SPAN: u32 = 4;

/// How far ONE reading of an invocation's clock moves it.
///
/// The span above is spent by what the RUN does, and a traced event is one of those: the
/// engine reads the run's clock once per event the agent delivers and once per sample the
/// host's watch takes over it.
///
/// DERIVED, not chosen: the whole step budget, at every reading one traced event takes of the
/// clock, spends the stated SHARE of the invocation's span and no more. So a run that spends
/// the traced events it is allowed is answered on that count — what it DID — and the span is
/// left to be spent by the waits the agent itself asks for, which are the other thing a run
/// spends its own time on. A tick that let the step budget spend the whole span would make
/// which of the two bounds answers an accident of how many times the engine happens to read
/// the clock per event.
pub const AGENT_RUN_CLOCK_TICK: Duration = match AGENT_RUN_WALL_CLOCK.checked_div(
    AGENT_RUN_STEPS
        .saturating_mul(CLOCK_READINGS_PER_TRACED_EVENT)
        .saturating_mul(STEP_BUDGET_SHARE_OF_THE_SPAN),
) {
    Some(tick) => tick,
    // Unreachable while the divisor above is a count of readings: a clock that stood still
    // would never reach a deadline even the waits an agent asked for had spent.
    None => Duration::from_nanos(1),
};

/// The CLOCK one invocation of a deployed agent is bounded against.
///
/// A clock the RUN's own readings drive, so the span the invocation is given is spent by what
/// the agent did — the traced events it delivers and the waits it asks for — and by no span of
/// this host's time: the interpreter one invocation runs in is built for it and the deployed
/// code's own modules are read in it, work a loaded host may need several times as long over,
/// and none of it is the agent's doing. What the run may take of THIS host is stated on the
/// clock beside it ([`AGENT_RUN_HOST_CEILING`]), which is the bound that holds the CPU one run
/// of caller-supplied code may take.
#[must_use]
pub fn agent_run_clock() -> python::Clock {
    python::Clock::driven(AGENT_RUN_CLOCK_TICK).within(AGENT_RUN_HOST_CEILING)
}

/// Every span the ENGINE spends on one invocation, in the run's own time: building the
/// execution environment the agent runs in ([`ENVIRONMENT_BUILD_SPAN`]), reading the deployed
/// code's own modules in it ([`INIT_PHASE_LIMIT`]) and then the invocation itself
/// ([`AGENT_RUN_WALL_CLOCK`]).
///
/// The first two are this EMULATOR's own work and not the agent's — an interpreter is built
/// and the deployment's modules are read before a line of the entry point runs — and the
/// engine charges them to no bound of this host's. So they are what the host time one
/// invocation may take has to cover: a ceiling under this sum stops an invocation the engine
/// has not finished standing up yet.
const ENGINE_SPANS_PER_RUN: Duration = ENVIRONMENT_BUILD_SPAN
    .saturating_add(INIT_PHASE_LIMIT)
    .saturating_add(AGENT_RUN_WALL_CLOCK);

/// How much of THIS HOST's own time ONE invocation may take, past which the engine stops it
/// whatever the run's own clock reads.
///
/// CHOSEN: the span above is the RUN's own time, so it bounds no CPU of this host — an agent
/// spending traced events, or sitting in one long native call that delivers none, would hold
/// a thread of this process for as long as the host took over it. This is the bound that
/// stops it: the engine reads it from this host's own clock at every traced event and from
/// the host's own watch over the run ([`python::Clock::within`]).
///
/// It is generous beside EVERY span the engine spends on one invocation
/// ([`ENGINE_SPANS_PER_RUN`]) — the environment it builds for the run and the deployed code's
/// imports it reads in it, which is where most of an invocation's host time goes, as well as
/// the invocation's own span — because a busy host may need several times as long of its own
/// for the same work, and an invocation the deployed code ANSWERED has to be reported on that
/// answer rather than on this host's load.
pub const AGENT_RUN_HOST_CEILING: Duration =
    ENGINE_SPANS_PER_RUN.saturating_mul(HOST_CEILING_OVER_THE_ENGINE_SPAN);

/// How many times the engine's own spans the host ceiling allows for
/// ([`AGENT_RUN_HOST_CEILING`]).
const HOST_CEILING_OVER_THE_ENGINE_SPAN: u32 = 3;

/// How much longer than the host time one run may take it is waited for before it is given up
/// on.
///
/// CHOSEN: a run stops at the next event its interpreter delivers after a bound was latched,
/// and the interpreter is then finalized, so the answer of a stopped run arrives a little
/// after that bound rather than at it. Past this the call is answered instead of waiting for
/// an agent that is not coming back.
pub const AGENT_RUN_GRACE: Duration = Duration::from_secs(5);

/// How long this invocation waits for the thread the run happens on: the host time one run
/// may take ([`AGENT_RUN_HOST_CEILING`]) and the grace the engine's own stop needs to land.
///
/// DERIVED, not chosen: a run that is going to answer at all has answered by then, because
/// every span the engine spends — standing the interpreter up, reading the deployed code's
/// modules and running the invocation — is inside the ceiling the run is held to. A wait of
/// the run's own span alone would give up on a run whose own bounds had not been reached yet,
/// and report an agent that was still being stood up as one that did not answer in time.
pub const AGENT_RUN_HOST_WAIT: Duration = AGENT_RUN_HOST_CEILING.saturating_add(AGENT_RUN_GRACE);

/// How many traced events one invocation may spend, how much memory it may add to this
/// process, how much of its output is kept, how large its answer may be and how deep it may
/// recurse.
///
/// Every one of them is the EMULATOR's: the agent is caller-supplied code, so what one
/// invocation may cost is stated here and enforced by the engine inside the run, in host
/// code the agent cannot reach.
#[must_use]
pub fn agent_run_budget() -> Budget {
    Budget {
        wall_clock: AGENT_RUN_WALL_CLOCK,
        steps: u64::from(AGENT_RUN_STEPS),
        memory_bytes: 256 * 1024 * 1024,
        // The agent sees no writable directory of its own beyond what the engine serves one,
        // so this bounds what a run that writes files can make this process hold.
        ephemeral_bytes: 64 * 1024 * 1024,
        log_bytes: 64 * 1024,
        // The answer carries the body the agent produced, so it is the response cap that
        // bounds what one invocation can make this process hold.
        response_bytes: 4 * 1024 * 1024,
        recursion_limit: 2_000,
    }
}

/// One invocation to deliver: the deployed code, the entry point it is delivered through,
/// and everything the request carried.
#[derive(Debug)]
pub struct AgentRun<'a> {
    /// The artifact's own modules, as object storage holds them.
    pub modules: &'a [PackageModule],
    /// The artifact's `entryPoint`: the file the invocation is delivered to, and the name of
    /// the callable in it when the deployment names one.
    pub entry_point: &'a [String],
    /// The request's `payload`, as the caller sent it.
    pub payload: &'a [u8],
    /// The `contentType` the caller stated for it, which decides how the payload reaches the
    /// agent.
    pub content_type: Option<&'a str>,
    /// The `accept` the caller asked the answer in.
    pub accept: Option<&'a str>,
    /// The `runtimeSessionId` the invocation runs under.
    pub session_id: &'a str,
    /// The `runtimeUserId` the invocation was made on behalf of, when the caller named one.
    pub user_id: Option<&'a str>,
    /// The ARN the invocation addressed.
    pub invoked_arn: &'a str,
    /// The environment the runtime version configures its agent with.
    pub environment: &'a BTreeMap<String, String>,
}

/// What a deployed agent answered.
#[derive(Debug, Clone)]
pub struct AgentAnswer {
    /// The bytes the agent produced, which is the invocation's `response` payload.
    pub body: Vec<u8>,
    /// The media type they are in, which is the invocation's `contentType`.
    pub content_type: String,
    /// The HTTP status the deployed code answered with, which the invocation reports as its
    /// own `statusCode` ("The HTTP status code of the response. A status code of 200
    /// indicates a successful operation. Other status codes indicate various error
    /// conditions").
    pub status: u16,
}

/// The status an answer that stated none is reported with: the one this operation's route
/// states as its success code.
pub const ANSWERED_STATUS: u16 = 200;

/// Why an invocation carries no answer from the agent.
#[derive(Debug, Clone)]
pub enum AgentRunFailure {
    /// The agent — or the contract resolving its entry point — raised.
    Raised {
        error_type: String,
        error_message: String,
    },
    /// The run was stopped by one of its bounds.
    Stopped(String),
    /// Nothing ran: the interpreter could not be stood up, or its answer could not be read.
    NotRun(String),
}

/// Deliver `run` to the deployed agent and answer with what it produced.
///
/// Nothing of the run happens on the async executor and nothing of it happens under this
/// service's state guard: the interpreter is synchronous and CPU-bound, so it runs on a
/// thread of its own and this call awaits its answer under a ceiling.
pub async fn run_agent(run: &AgentRun<'_>) -> Result<AgentAnswer, AgentRunFailure> {
    let event = match run_event(run) {
        Ok(event) => event,
        Err(reason) => return Err(AgentRunFailure::NotRun(reason)),
    };
    // The artifact's own modules first, then the two this emulator supplies: the contract the
    // invocation is delivered by, and the app package the agent registered its entry point
    // with, so a vendored copy of that package is not what serves the invocation.
    let mut modules = run.modules.to_vec();
    modules.push(PackageModule {
        path: APP_PACKAGE.to_string(),
        source: APP_PACKAGE_SOURCE.to_string(),
    });
    modules.push(PackageModule {
        path: APP_RUNTIME_MODULE.to_string(),
        source: APP_SOURCE.to_string(),
    });
    modules.push(PackageModule {
        path: CONTRACT_MODULE.to_string(),
        source: CONTRACT_SOURCE.to_string(),
    });
    let environment = run.environment.clone();
    let session = run.session_id.to_string();
    let invoked_arn = run.invoked_arn.to_string();
    let budget = agent_run_budget();
    let (sender, receiver) = ::tokio::sync::oneshot::channel();
    let spawned = ::std::thread::Builder::new()
        .name("agentcore-runtime-invocation".to_string())
        .stack_size(RUN_STACK_BYTES)
        .spawn(move || {
            let finished = python::run_against(
                &python::Run {
                    modules: &modules,
                    handler: CONTRACT_HANDLER,
                    environment: &environment,
                    // The instruction set the environment the agent runs in has, which the
                    // run reports as its own machine. A runtime names none, so it is the one
                    // the engine's own runtime reports when none is named.
                    architecture:
                        ::service_com_amazonaws_lambda_awsgirapiservice::state::DEFAULT_ARCHITECTURE,
                    event: Some(&event),
                    // The session the invocation runs under is what identifies it to the
                    // agent, and the ARN it addressed is what the agent reads as the resource
                    // it is.
                    request_id: &session,
                    invoked_function_arn: &invoked_arn,
                    client_context: None,
                    tenant_id: None,
                    cognito_identity: None,
                    budget,
                    // The agent reaches no other service of this emulator: this run is given
                    // no seam to dispatch an AWS call on.
                    aws: None,
                    // It is no Lambda durable execution's run, so it takes no step of one.
                    durable: None,
                    // And it mounts no network file system: the only files it reads are the
                    // artifact's own.
                    file_systems: &[],
                },
                // The clock the run's own span is read against: one the RUN drives, a tick
                // per traced event it delivers, stating what the run may take of THIS host
                // besides ([`agent_run_clock`]).
                &agent_run_clock(),
            );
            let _ = sender.send(finished);
        });
    if spawned.is_err() {
        return Err(AgentRunFailure::NotRun(
            "the deployed agent could not be given a thread to run on".to_string(),
        ));
    }
    let waited = ::tokio::time::timeout(AGENT_RUN_HOST_WAIT, receiver).await;
    let finished = match waited {
        Ok(Ok(finished)) => finished,
        // The run was given up on, or its thread ended without reporting: the caller is
        // answered now rather than left waiting for an agent that is not coming back.
        Ok(Err(_)) | Err(_) => {
            return Err(AgentRunFailure::Stopped(stopped_reason(Stop::HostCeiling)))
        }
    };
    match &finished.completion {
        // The engine's LATCH: a run whose budget was spent is a stopped run whatever the
        // agent went on to return or catch, so the bound is what this reports.
        Completion::Stopped(stop) => Err(AgentRunFailure::Stopped(stopped_reason(*stop))),
        Completion::Returned(answer) => answered(answer),
        Completion::Raised {
            error_type,
            error_message,
            ..
        } => Err(AgentRunFailure::Raised {
            error_type: error_type.clone(),
            error_message: error_message.clone(),
        }),
        Completion::Internal(reason) => Err(AgentRunFailure::NotRun(reason.clone())),
        Completion::Streamed { .. } => Err(AgentRunFailure::NotRun(
            "the deployed agent's answer was not the one the runtime contract reports"
                .to_string(),
        )),
    }
}

/// How a run the engine STOPPED is reported, in the words of the bound it ran past.
fn stopped_reason(stop: Stop) -> String {
    match stop {
        Stop::Deadline => format!(
            "the deployed agent did not answer within {} seconds",
            AGENT_RUN_WALL_CLOCK.as_secs()
        ),
        // Not a span of the agent's own: this is the run having been given up on, so it says
        // so rather than reporting a bound of the invocation that never ran out.
        Stop::HostCeiling => "the invocation of the deployed agent was given up on".to_string(),
        Stop::Steps => "the deployed agent ran past the step budget of one invocation".to_string(),
        Stop::Memory => {
            "the deployed agent took more memory than one invocation may".to_string()
        }
        Stop::Response => {
            "the deployed agent's answer is larger than one invocation may return".to_string()
        }
    }
}

/// The EVENT one invocation is handed: the entry point it is delivered through, the
/// request's payload and the media types it names, and what identifies the invocation.
///
/// The payload goes in as base64, so a request body whose bytes are not text reaches the
/// agent exactly as the caller sent it.
fn run_event(run: &AgentRun<'_>) -> Result<String, String> {
    let event = ::serde_json::json!({
        "entryPoint": run.entry_point,
        "payload": ::aws_smithy_types::base64::encode(run.payload),
        "contentType": run.content_type.unwrap_or_default(),
        "accept": run.accept.unwrap_or_default(),
        "sessionId": run.session_id,
        "userId": run.user_id.unwrap_or_default(),
        "runtimeArn": run.invoked_arn,
    });
    ::serde_json::to_string(&event)
        .map_err(|error| format!("the invocation could not be handed to the agent: {error}"))
}

/// What the contract ANSWERED, as the answer this operation reports.
fn answered(answer: &str) -> Result<AgentAnswer, AgentRunFailure> {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(answer) else {
        return Err(AgentRunFailure::NotRun(
            "the deployed agent's answer could not be read".to_string(),
        ));
    };
    let text = |member: &str| {
        document
            .get(member)
            .and_then(::serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let Ok(body) = ::aws_smithy_types::base64::decode(text("body")) else {
        return Err(AgentRunFailure::NotRun(
            "the deployed agent's answer could not be read".to_string(),
        ));
    };
    Ok(AgentAnswer {
        body,
        content_type: text("contentType"),
        status: document
            .get("statusCode")
            .and_then(::serde_json::Value::as_u64)
            .and_then(|status| u16::try_from(status).ok())
            .filter(|status| (100..=599).contains(status))
            .unwrap_or(ANSWERED_STATUS),
    })
}

/// The top-level module of the app package a deployed agent imports, which re-exports what
/// the AgentCore app serves an invocation through — the names an agent reads off the package
/// itself.
const APP_PACKAGE_SOURCE: &str = r#"
"""The AgentCore Runtime app package a deployed agent serves its invocations through."""

from .runtime import BedrockAgentCoreApp, RequestContext

__all__ = ["BedrockAgentCoreApp", "RequestContext"]
"#;

/// The APP a deployed agent registers its entry point with.
///
/// On AWS this package stands up an HTTP server in the agent's container and serves
/// `POST /invocations` through the function the agent decorated. Here the HOST delivers the
/// invocation, so the app records that function and its `run` starts nothing: there is no
/// socket to open, and the contract module below calls what was registered.
const APP_SOURCE: &str = r#"
"""The AgentCore Runtime app: what a deployed agent registers its entry point with.

The invocation is delivered by the HOST, which calls the registered entry point directly,
so nothing here opens a socket or starts a server. `run` exists because a deployed agent
calls it at the end of its module and must not block: the module that called it has already
registered everything the invocation needs.
"""

_registered = []


class RequestContext:
    """What the invocation being served is, as the entry point reads it."""

    def __init__(self, session_id=None, request_id=None, user_id=None, **held):
        self.session_id = session_id
        self.request_id = request_id
        self.user_id = user_id
        for name, value in held.items():
            setattr(self, name, value)


class BedrockAgentCoreApp:
    """The app a deployed agent serves its invocations through."""

    def __init__(self, debug=False, **held):
        self.debug = debug
        self.handler = None
        for name, value in held.items():
            setattr(self, name, value)

    def entrypoint(self, function):
        """Register the function one invocation is delivered to."""
        self.handler = function
        _registered.append(function)
        return function

    def ping(self, function):
        """Register the health reading. Nothing polls it here: no server is standing."""
        self.health = function
        return function

    def async_task(self, function):
        """Register a task the agent runs beside a turn, which starts nothing here."""
        return function

    def run(self, port=8080, host=None):
        """Serve invocations.

        The host serves them, so this returns at once instead of blocking on a socket.
        """
        self.port = port
        self.host = host
        return None


def registered_entrypoint():
    """The entry point the deployed code registered, or None when it registered none."""
    if not _registered:
        return None
    return _registered[-1]
"#;

/// The CONTRACT one invocation resolves: the module that imports the artifact's entry point,
/// delivers the request's payload to it and answers with what it returned.
///
/// Ordinary Python, the way every module this workspace supplies an interpreter is. It is
/// NOT the containment — that is the engine's, in host code the agent cannot reach — it is
/// the DELIVERY: which callable an invocation goes to, what the payload reaches it as, and
/// what the body and media type of the answer are.
const CONTRACT_SOURCE: &str = r#"
"""The AgentCore Runtime contract one invocation of a deployed agent is delivered through."""

import base64
import importlib
import inspect
import io
import json

import bedrock_agentcore
from bedrock_agentcore.runtime import RequestContext


# The media type an answer is reported in when the request asked for none: the format an
# agent's JSON answer is in.
_JSON_MEDIA_TYPE = "application/json"

# The media type a STREAMING answer is reported in: an entry point that yields its answer
# serves it as server-sent events, one event per chunk it yielded.
_EVENT_STREAM_MEDIA_TYPE = "text/event-stream"

# The route AWS delivers an invocation of an agent runtime to, and the method it delivers it
# with: the deployed code serves `POST /invocations`.
_INVOCATIONS_ROUTE = "/invocations"

# The header the session an invocation runs under reaches the deployed code in.
_SESSION_HEADER = "X-Amzn-Bedrock-AgentCore-Runtime-Session-Id"

# The port an agent runtime's own server is addressed on, which is what the code reads off
# the listener it was given.
_AGENT_PORT = 8080

# Every handler class the deployed code stood a server up with, newest last.
_stood_up = []


def _module_of(entry):
    """The module name the entry point's file names."""
    name = str(entry).strip().lstrip("/")
    if name.endswith(".py"):
        name = name[:-3]
    return name.replace("/", ".").strip(".")


class _Listener:
    """The LISTENER the host is, in place of the one the deployed code would open.

    AWS runs the deployed code in a container and delivers each invocation to the server
    that code stands up. Here the HOST delivers it: the code's own request handler is driven
    over an in-memory connection ([`_Connection`]), so standing the server up records the
    handler class and binds nothing — no socket is opened, and serving returns at once
    instead of waiting for a connection that no listener could accept.
    """

    def __init__(self, server_address=None, RequestHandlerClass=None, bind_and_activate=True):
        self.server_address = tuple(server_address or ("0.0.0.0", _AGENT_PORT))
        self.RequestHandlerClass = RequestHandlerClass
        self.server_name = "localhost"
        self.server_port = (
            self.server_address[1] if len(self.server_address) > 1 else _AGENT_PORT
        )
        self.socket = None
        if RequestHandlerClass is not None:
            _stood_up.append(RequestHandlerClass)

    def serve_forever(self, poll_interval=0.5):
        return None

    def handle_request(self):
        return None

    def shutdown(self):
        return None

    def server_close(self):
        return None

    def fileno(self):
        raise OSError("this agent runtime's listener is the host, which has no descriptor")

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback_):
        return False


def _host_the_listener():
    """Put [`_Listener`] in place of the server classes a deployed agent stands up.

    Done BEFORE the entry module is imported, so code that stands its server up at import —
    or inside the `__main__` guard a container would run it under — registers its handler and
    returns rather than waiting on a socket for ever.
    """
    import http.server
    import socketserver

    for module, names in (
        (http.server, ("HTTPServer", "ThreadingHTTPServer")),
        (socketserver, ("TCPServer", "ThreadingTCPServer")),
    ):
        for name in names:
            if hasattr(module, name):
                setattr(module, name, _Listener)


class _Sink(io.BytesIO):
    """An in-memory stream whose close keeps what was written: the request handler closes
    both ends of its connection when it has finished, and what it wrote is the answer."""

    def close(self):
        return None


class _Connection:
    """The CONNECTION one invocation is delivered over: the request bytes to be read, and
    the bytes the deployed code's handler writes back.

    It is the object a request handler is handed in place of a socket, and it answers the
    calls `socketserver`'s own stream handler makes on one. Nothing of the host is reachable
    through it: there is no descriptor, and nothing is sent anywhere.
    """

    def __init__(self, request):
        self._incoming = _Sink(request)
        self._outgoing = _Sink()

    def makefile(self, mode="rb", bufsize=-1, *args, **held):
        if "w" in mode or "a" in mode:
            return self._outgoing
        return self._incoming

    def sendall(self, data):
        self._outgoing.write(bytes(data))
        return None

    def send(self, data):
        self._outgoing.write(bytes(data))
        return len(data)

    def settimeout(self, timeout):
        return None

    def gettimeout(self):
        return None

    def shutdown(self, how):
        return None

    def close(self):
        return None

    def fileno(self):
        raise OSError("this connection is in memory and has no descriptor")

    def answered(self):
        return self._outgoing.getvalue()


def _handler_class(module):
    """The HTTP request handler the deployed code serves its invocations with."""
    try:
        from http.server import BaseHTTPRequestHandler
    except ImportError:
        return None
    if _stood_up:
        return _stood_up[-1]
    fallback = None
    for name in dir(module):
        held = getattr(module, name, None)
        if not isinstance(held, type) or held is BaseHTTPRequestHandler:
            continue
        if not issubclass(held, BaseHTTPRequestHandler):
            continue
        if getattr(held, "__module__", "") == getattr(module, "__name__", ""):
            return held
        if fallback is None:
            fallback = held
    return fallback


def _request_bytes(event, payload):
    """The invocation as the HTTP request AWS delivers to the deployed code."""
    headers = [
        ("Host", "localhost:%d" % _AGENT_PORT),
        ("Content-Type", (event.get("contentType") or "").strip() or _JSON_MEDIA_TYPE),
        ("Content-Length", str(len(payload))),
        (_SESSION_HEADER, str(event.get("sessionId") or "")),
        # One invocation per connection: the handler's own loop ends with this request
        # rather than waiting for another on a connection nothing else writes to.
        ("Connection", "close"),
    ]
    accept = (event.get("accept") or "").strip()
    if accept:
        headers.append(("Accept", accept))
    user = (event.get("userId") or "").strip()
    if user:
        headers.append(("X-Amzn-Bedrock-AgentCore-Runtime-User-Id", user))
    head = "POST %s HTTP/1.1\r\n" % _INVOCATIONS_ROUTE
    for name, value in headers:
        head += "%s: %s\r\n" % (name, value)
    head += "\r\n"
    return head.encode("latin-1") + payload


def _answered_over_http(raw):
    """The status, media type and body of the response the deployed code wrote."""
    head, separator, body = raw.partition(b"\r\n\r\n")
    if not separator:
        head, body = b"", raw
    lines = head.split(b"\r\n")
    status = 200
    if lines and lines[0].startswith(b"HTTP/"):
        stated = lines[0].split()
        if len(stated) > 1 and stated[1].isdigit():
            status = int(stated[1])
    content_type = ""
    for line in lines[1:]:
        name, separator, value = line.partition(b":")
        if not separator:
            continue
        if name.strip().lower() == b"content-type":
            content_type = value.strip().decode("latin-1")
    return status, content_type, body


def _over_http(handler_class, event, payload):
    """Deliver the invocation to the handler the deployed code serves `POST /invocations`
    with, and answer with the response that handler wrote."""
    connection = _Connection(_request_bytes(event, payload))
    handler_class(connection, ("127.0.0.1", 0), _Listener())
    return _answered_over_http(connection.answered())


def _delivered_to(module, entry):
    """What this invocation is delivered to: the HTTP request handler the deployed code
    serves `POST /invocations` with, the entry point it registered with the AgentCore app, or
    the callable its own `entryPoint` names."""
    named = str(entry[1]).strip() if len(entry) > 1 else ""
    if named:
        held = getattr(module, named, None)
        if callable(held):
            return "call", held
    registered = bedrock_agentcore.runtime.registered_entrypoint()
    if registered is not None:
        return "call", registered
    serving = _handler_class(module)
    if serving is not None:
        return "http", serving
    raise RuntimeError(
        "the deployed code at %r serves no entry point for an invocation: it stood up no "
        "server on %s, registered nothing with the AgentCore app, and its entry point names "
        "no callable of its own" % (entry[0], _INVOCATIONS_ROUTE)
    )


def _payload_of(event, raw):
    """The request's payload as a CALLABLE entry point reads it.

    A payload the request stated a JSON media type for (or stated none for) is handed over
    as the value it parses to, which is what an agent's entry point takes; anything else is
    handed over as the text it is, and bytes that are not text as those bytes.
    """
    stated = (event.get("contentType") or "").split(";")[0].strip().lower()
    if stated == "" or stated == _JSON_MEDIA_TYPE or stated.endswith("+json"):
        try:
            return json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, ValueError):
            pass
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return raw


def _called(delivered, payload, context):
    """Call the entry point the way its own signature takes an invocation."""
    try:
        parameters = list(inspect.signature(delivered).parameters.values())
    except (TypeError, ValueError):
        return delivered(payload)
    kinds = [parameter.kind for parameter in parameters]
    if inspect.Parameter.VAR_POSITIONAL in kinds:
        return delivered(payload, context)
    positional = [
        kind
        for kind in kinds
        if kind
        in (inspect.Parameter.POSITIONAL_ONLY, inspect.Parameter.POSITIONAL_OR_KEYWORD)
    ]
    if len(positional) >= 2:
        return delivered(payload, context)
    if len(positional) == 1:
        return delivered(payload)
    return delivered()


def _answered(value, accept):
    """The body and media type the entry point's own answer is reported as."""
    if isinstance(value, (bytes, bytearray)):
        return bytes(value), accept or _JSON_MEDIA_TYPE
    if isinstance(value, str):
        return value.encode("utf-8"), accept or _JSON_MEDIA_TYPE
    if hasattr(value, "__iter__") and not isinstance(value, (dict, list, tuple, set)):
        framed = []
        for chunk in value:
            if isinstance(chunk, (bytes, bytearray)):
                chunk = bytes(chunk).decode("utf-8", "replace")
            elif not isinstance(chunk, str):
                chunk = json.dumps(chunk)
            framed.append("data: " + chunk + "\n\n")
        return "".join(framed).encode("utf-8"), _EVENT_STREAM_MEDIA_TYPE
    return json.dumps(value).encode("utf-8"), accept or _JSON_MEDIA_TYPE


def invoke(event, context):
    entry = event.get("entryPoint") or []
    if not entry:
        raise RuntimeError("the agent runtime names no entry point to deliver the invocation to")
    payload = base64.b64decode(event.get("payload") or "")
    # Before a line of the deployed code runs: the listener is the host's, so a server the
    # code stands up registers its handler instead of waiting on a socket.
    _host_the_listener()
    module = importlib.import_module(_module_of(entry[0]))
    how, delivered = _delivered_to(module, entry)
    if how == "http":
        status, content_type, body = _over_http(delivered, event, payload)
        return {
            "body": base64.b64encode(body).decode("ascii"),
            "contentType": content_type or _JSON_MEDIA_TYPE,
            "statusCode": status,
        }
    answer = _called(
        delivered,
        _payload_of(event, payload),
        RequestContext(
            session_id=event.get("sessionId") or None,
            request_id=event.get("sessionId") or None,
            user_id=event.get("userId") or None,
        ),
    )
    body, content_type = _answered(answer, (event.get("accept") or "").strip())
    return {
        "body": base64.b64encode(body).decode("ascii"),
        "contentType": content_type,
        "statusCode": 200,
    }
"#;
