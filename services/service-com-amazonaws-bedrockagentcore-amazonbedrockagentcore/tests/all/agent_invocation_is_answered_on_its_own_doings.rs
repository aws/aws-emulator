// SPDX-License-Identifier: CC0-1.0

//! Which bound answers one invocation of a deployed agent runtime, and what the invocation
//! therefore answers.
//!
//! The rule under test: **a bound that stops an invocation of a deployed agent is spent by
//! what the RUN itself did — the traced events it delivers and the waits it asks for — so
//! neither this host's speed nor a reading the engine took of the run's clock of its own
//! accord spends the span one invocation is given, and an invocation the deployed code
//! ANSWERED is reported on that answer.** An invocation stopped on a span it never spent is
//! answered with the modelled `RuntimeClientError` instead of the body the deployed code
//! produced, which makes what a caller is answered a reading of how fast or how busy the host
//! happened to be.
//!
//! The rows differ along what the rule generalises over:
//!
//! * for the SPAN: how many readings of the invocation's clock are taken, which is what a
//!   clock the run drives moves with and what this host's own time must not move it by;
//! * for the CALIBRATION: how many times the engine reads that clock per traced event the run
//!   delivers, which is what the whole step budget's spend of the span is read against — so
//!   the bound that answers a run which spends the work it is allowed is the count of that
//!   work and never the span;
//! * for what an invocation ANSWERS: how the deployed code serves the invocation (the HTTP
//!   request handler it stands a server up with, an entry point it registered with the
//!   AgentCore app it imports, the callable its artifact's own `entryPoint` names, one that
//!   takes the invocation's context beside the payload) and what that code answers with (a
//!   success and a non-success status). Every row sends a marker of its own and asserts the
//!   answer carries what the code made of THAT marker, so a body this emulator composed
//!   itself cannot pass a row.
//!
//! The spans and the budget are the crate's own, so what is judged is the calibration an
//! invocation really runs under rather than a copy of it. Nothing here sleeps and nothing is
//! asserted about the wall clock: how long this host takes over a run is exactly what the rule
//! says is not the agent's doing.

use std::collections::BTreeMap;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;

/// The file every deployment below is named by.
const ENTRY_FILE: &str = "main.py";

/// A `runtimeSessionId` of the length that member admits (`SessionType`, 33..256).
const SESSION: &str = "deployed-agent-answer-session-001";

/// The ARN an invocation of these rows addresses.
const RUNTIME_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:000000000000:runtime/answeringAgent-abcdefghij";

/// A reading of the clock an invocation is bounded against moves it by one tick and by nothing
/// of this host's own time, so the span the invocation is given is spent by the run's own
/// doings alone.
///
/// A clock any of this host's time moved would answer a span that is not a whole number of
/// ticks, whatever the host's speed.
#[test]
fn the_invocations_own_span_is_moved_by_the_runs_readings_alone() {
    // (case, how many readings this row takes)
    let cases: &[(&str, u32)] = &[
        ("one reading", 1),
        ("two readings", 2),
        ("a few readings", 16),
        ("many readings", 4_096),
    ];
    for (case, readings) in cases {
        let clock = agentcore::state::agent_run_clock();
        let first = clock.now();
        let mut taken = first;
        for _ in 0..*readings {
            taken = clock.now();
        }
        let moved = taken.saturating_duration_since(first);
        let expected = agentcore::state::AGENT_RUN_CLOCK_TICK.saturating_mul(*readings);
        assert_eq!(
            moved, expected,
            "case {case:?}: expected {readings} readings of the invocation's clock to move it \
             by {expected:?}, got {moved:?}"
        );
    }
}

/// The span the invocation is given is the RUN's own span and no span of this host's, so the
/// clock hands the run the whole span it is configured for rather than holding it to what this
/// host may spend on a run bounded against the host's own clock.
#[test]
fn the_clock_hands_the_invocation_the_whole_span_it_is_configured_for() {
    let clock = agentcore::state::agent_run_clock();
    let span = clock.host_span(agentcore::state::AGENT_RUN_WALL_CLOCK);
    assert_eq!(
        span,
        agentcore::state::AGENT_RUN_WALL_CLOCK,
        "expected the invocation's own span of {:?} to stand, got {span:?}",
        agentcore::state::AGENT_RUN_WALL_CLOCK
    );
}

/// The whole step budget spends only part of the invocation's own span — at every number of
/// readings one traced event takes of the run's clock — so a run that spends the traced events
/// it is allowed is answered on that count, which is what it DID, and never on a span that did
/// not run out.
#[test]
fn the_whole_step_budget_spends_only_part_of_the_invocations_own_span() {
    let budget = agentcore::state::agent_run_budget();
    // How many times the engine reads the run's clock per traced event. One reading is the
    // floor; TWO is what the engine really takes (it reads the clock to compare the run with
    // its deadline, and reads it again for the span the run has spent of that deadline); the
    // larger rows are the room a reading taken anywhere else — the host's own watch over the
    // run, a blocking seam the run calls — has to have.
    let cases: &[(&str, u32)] = &[
        ("one reading per traced event", 1),
        ("the two readings the engine takes per traced event", 2),
        ("twice the readings the engine takes", 4),
        ("three times the readings the engine takes", 6),
    ];
    for (case, readings) in cases {
        let spent = agentcore::state::AGENT_RUN_CLOCK_TICK
            .saturating_mul(u32::try_from(budget.steps).unwrap_or(u32::MAX))
            .saturating_mul(*readings);
        assert!(
            spent < agentcore::state::AGENT_RUN_WALL_CLOCK,
            "case {case:?}: the whole step budget of {} traced events spends {spent:?} of the \
             invocation's span, which must stay under {:?}",
            budget.steps,
            agentcore::state::AGENT_RUN_WALL_CLOCK
        );
    }
}

/// An agent that answers from the callable its artifact's own `entryPoint` names.
fn naming_its_own_callable() -> String {
    r#"
def answer(payload):
    return {"answer": payload["prompt"].upper(), "served_by": "entrypoint"}
"#
    .to_string()
}

/// An agent that answers from the entry point it registered with the AgentCore app it imports,
/// and whose module then calls the app's own `run` as a deployed agent's module does.
fn registered_with_the_app() -> String {
    r#"
from bedrock_agentcore.runtime import BedrockAgentCoreApp

app = BedrockAgentCoreApp()


@app.entrypoint
def invoke(payload):
    return {"answer": payload["prompt"].upper(), "served_by": "app"}


app.run()
"#
    .to_string()
}

/// An agent whose callable takes the invocation's CONTEXT beside the payload, and reports the
/// session the invocation runs under so only a run the context really reached can pass.
fn taking_the_invocations_context() -> String {
    r#"
def answer(payload, context):
    return {
        "answer": payload["prompt"].upper(),
        "served_by": "context:" + str(context.session_id),
    }
"#
    .to_string()
}

/// An agent that serves the invocation route itself, with its own HTTP request handler, and
/// answers `status` with what it made of the request's payload.
fn serving_the_route(status: u16) -> String {
    format!(
        r#"
import json
from http.server import BaseHTTPRequestHandler, HTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        request = json.loads(self.rfile.read(length) or b"{{}}")
        answer = json.dumps(
            {{"answer": request["prompt"].upper(), "served_by": "http"}}
        ).encode("utf-8")
        self.send_response({status})
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(answer)))
        self.end_headers()
        self.wfile.write(answer)

    def log_message(self, template, *held):
        return None


if __name__ == "__main__":
    HTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
"#
    )
}

/// Deliver ONE invocation of `source` to the deployed-agent run path, exactly as the operation
/// does. `named` is the callable the artifact's `entryPoint` names, where the deployment names
/// one.
async fn invoked(
    source: String,
    named: Option<String>,
    marker: String,
) -> Result<agentcore::state::AgentAnswer, agentcore::state::AgentRunFailure> {
    let modules = vec![PackageModule {
        path: ENTRY_FILE.to_string(),
        source,
    }];
    let mut entry_point = vec![ENTRY_FILE.to_string()];
    entry_point.extend(named);
    let payload = ::serde_json::json!({"prompt": marker}).to_string();
    let environment = BTreeMap::new();
    agentcore::state::run_agent(&agentcore::state::AgentRun {
        modules: &modules,
        entry_point: &entry_point,
        payload: payload.as_bytes(),
        content_type: Some("application/json"),
        accept: Some("application/json"),
        session_id: SESSION,
        user_id: None,
        invoked_arn: RUNTIME_ARN,
        environment: &environment,
    })
    .await
}

/// ONE row: the deployed code, how its entry point is addressed, and what that code answers.
struct Deployment {
    /// What this row deploys.
    case: &'static str,
    /// The deployed code itself.
    source: String,
    /// The callable the artifact's `entryPoint` names, for a deployment that names one.
    named: Option<String>,
    /// The marker this row's request carries, which the answer has to be made of.
    marker: &'static str,
    /// The status the deployed code answers with.
    status: u16,
    /// What the code reports as having served the invocation.
    served_by: String,
}

/// Whatever host time the engine spends standing an agent up and reading its modules, an
/// invocation is answered with what the deployed code served — in every form the deployed code
/// can serve it in, and with the status that code answered.
#[tokio::test]
async fn an_invocation_is_answered_with_what_the_deployed_code_served() {
    let cases: Vec<Deployment> = vec![
        Deployment {
            case: "a callable the artifact's own entryPoint names",
            source: naming_its_own_callable(),
            named: Some("answer".to_string()),
            marker: "named-callable-marker",
            status: 200,
            served_by: "entrypoint".to_string(),
        },
        Deployment {
            case: "an entry point registered with the AgentCore app",
            source: registered_with_the_app(),
            named: None,
            marker: "registered-entrypoint-marker",
            status: 200,
            served_by: "app".to_string(),
        },
        Deployment {
            case: "a callable that takes the invocation's context beside the payload",
            source: taking_the_invocations_context(),
            named: Some("answer".to_string()),
            marker: "context-marker",
            status: 200,
            served_by: format!("context:{SESSION}"),
        },
        Deployment {
            case: "an HTTP request handler the code serves the invocation route with",
            source: serving_the_route(200),
            named: None,
            marker: "served-route-marker",
            status: 200,
            served_by: "http".to_string(),
        },
        Deployment {
            case: "an HTTP request handler that answers a non-success status",
            source: serving_the_route(503),
            named: None,
            marker: "refused-marker",
            status: 503,
            served_by: "http".to_string(),
        },
    ];
    // The rows are delivered together: each run happens on a thread of its own, so the suite
    // costs about one invocation's host time rather than one per row.
    let mut running = Vec::new();
    for row in cases {
        let Deployment {
            case,
            source,
            named,
            marker,
            status,
            served_by,
        } = row;
        let invoking = ::tokio::spawn(invoked(source, named, marker.to_string()));
        running.push((case, marker, status, served_by, invoking));
    }
    for (case, marker, status, served_by, invoking) in running {
        let answered = invoking.await.expect("the invocation's task ended");
        let answer = match answered {
            Ok(answer) => answer,
            Err(failure) => panic!(
                "case {case:?}: expected the answer the deployed code served for {marker:?}, \
                 got {failure:?}"
            ),
        };
        assert_eq!(
            answer.status,
            status,
            "case {case:?}: expected the status the deployed code answered ({status}), got {} \
             with {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let document: ::serde_json::Value =
            ::serde_json::from_slice(&answer.body).unwrap_or(::serde_json::Value::Null);
        assert_eq!(
            document["answer"].as_str(),
            Some(marker.to_uppercase().as_str()),
            "case {case:?}: expected the answer the code made of {marker:?}, got {document:?}"
        );
        assert_eq!(
            document["served_by"].as_str(),
            Some(served_by.as_str()),
            "case {case:?}: expected the invocation to reach the entry point this row deploys \
             ({served_by:?}), got {document:?}"
        );
    }
}
