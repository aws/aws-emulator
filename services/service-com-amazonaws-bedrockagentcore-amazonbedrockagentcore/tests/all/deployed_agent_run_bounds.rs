// SPDX-License-Identifier: CC0-1.0

//! The BOUNDS one invocation of a deployed agent runtime is held to, and what an invocation
//! therefore answers.
//!
//! The rule under test: **a bound this emulator holds an invocation of caller-supplied agent
//! code to is spent by what the RUN did, so the span of THIS HOST's time such a run may take
//! is no bound on the agent at all — it is the backstop for a run that delivers no traced
//! event, and it therefore has to exceed EVERY span the engine spends on one invocation,
//! including the execution environment it builds for it and the deployed modules it reads in
//! that environment, with room for a host that is slower or busier.** A host span under that
//! sum stops an invocation the engine has not finished standing up yet, so what the caller is
//! answered would be decided by this host's speed rather than by what the deployed code did.
//!
//! The rows differ along what the rule generalises over: which host-side span is being
//! judged (the ceiling the engine stops a run on, the wait this service holds a caller's call
//! for) and which span it has to cover; and, for what an invocation ANSWERS, how much HOST
//! work the deployed code costs beside the span of its own that it spends (an agent that
//! answers from its entry point, one that first spends real work inside native calls that
//! deliver almost no traced event, one that asks to wait longer than one invocation's own
//! span). Each answering row carries a marker of its own and asserts the answer is what the
//! code made of THAT marker, so no body this emulator composed could pass it.
//!
//! The spans are the crate's own (`state::AGENT_RUN_HOST_CEILING` and friends), so what is
//! judged is the calibration an invocation really runs under rather than a copy of it.
//! Nothing here waits a fixed time and nothing is asserted about the wall clock: how long
//! this host takes over a run is exactly what the rule says is not the agent's doing, and a
//! wait an agent itself asks for is charged to the invocation's own span with no host time
//! passing over it.

use std::collections::BTreeMap;
use std::time::Duration;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    ENVIRONMENT_BUILD_SPAN, INIT_PHASE_LIMIT,
};

/// The file a deployment's `entryPoint` names in every row below.
const ENTRY_FILE: &str = "main.py";

/// A `runtimeSessionId` of the length that member admits (`SessionType`, 33..256).
const SESSION: &str = "deployed-agent-bounds-session-001";

/// The ARN an invocation of these rows addresses.
const RUNTIME_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:000000000000:runtime/boundedAgent-abcdefghij";

/// Every span the ENGINE spends on ONE invocation, in the run's own time: the execution
/// environment it builds for the agent, the deployed code's own imports read in it, and then
/// the invocation itself. The first two are this emulator's own work on whatever host it is
/// running on, which is why the host-side spans have to cover them.
fn engine_spans_per_run() -> Duration {
    ENVIRONMENT_BUILD_SPAN
        .saturating_add(INIT_PHASE_LIMIT)
        .saturating_add(agentcore::state::AGENT_RUN_WALL_CLOCK)
}

/// Each host-side span one invocation is bounded by exceeds the spans it has to cover, so an
/// invocation is given up on only when the engine's own work for it has had every span the
/// engine allows that work — never while an agent is still being stood up.
#[test]
fn every_host_span_an_invocation_takes_exceeds_what_it_has_to_cover() {
    let engine_spans = engine_spans_per_run();
    // (case, the host-side span, what that span has to exceed, what the covered span is)
    let cases: &[(&str, Duration, Duration, &str)] = &[
        (
            "the host ceiling the engine stops an invocation on",
            agentcore::state::AGENT_RUN_HOST_CEILING,
            engine_spans,
            "every span the engine spends on one invocation",
        ),
        (
            "the wait this service holds the caller's call for",
            agentcore::state::AGENT_RUN_HOST_WAIT,
            engine_spans,
            "every span the engine spends on one invocation",
        ),
        (
            "the wait, against the ceiling the engine stops a run on",
            agentcore::state::AGENT_RUN_HOST_WAIT,
            agentcore::state::AGENT_RUN_HOST_CEILING,
            "the ceiling the engine stops a run on",
        ),
    ];
    for (case, held, covered, what) in cases {
        assert!(
            held > covered,
            "case {case:?}: expected {held:?} to exceed {what} ({covered:?}), got {held:?}"
        );
    }
}

/// The whole step budget spends less than one invocation's own span, so an agent that
/// computes without end is answered by a count of its OWN work and not by a span of time.
#[test]
fn the_step_budget_spends_less_than_one_invocations_own_span() {
    let budget = agentcore::state::agent_run_budget();
    let spent = agentcore::state::AGENT_RUN_CLOCK_TICK
        .saturating_mul(u32::try_from(budget.steps).unwrap_or(u32::MAX));
    assert!(
        spent < agentcore::state::AGENT_RUN_WALL_CLOCK,
        "case {:?}: the step budget spends {spent:?} of the invocation's own span, which must \
         stay under {:?}",
        "the whole step budget",
        agentcore::state::AGENT_RUN_WALL_CLOCK
    );
}

/// An agent that answers from the entry point its artifact names, with the marker the
/// request carried.
fn answering_agent() -> String {
    r#"
def answer(payload):
    return {"answer": payload["prompt"].upper(), "cost": "entrypoint"}
"#
    .to_string()
}

/// How many copies of the request's own marker the host-working agent builds its large
/// string out of, and how many items it sorts: enough native work to cost this host real
/// time, and far inside every bound one invocation is given.
const HOST_WORK_COPIES: usize = 4_096;
/// See [`HOST_WORK_COPIES`].
const HOST_WORK_SORTED_ITEMS: usize = 100_000;

/// An agent that first spends real HOST work inside native calls — building and measuring a
/// large string, and sorting a large list — which deliver almost no traced event, and only
/// then answers with what it computed over the request's own marker.
fn host_working_agent(copies: usize, sorted_items: usize) -> String {
    format!(
        r#"
def answer(payload):
    marker = payload["prompt"]
    held = marker * {copies}
    descending = sorted(range({sorted_items}), reverse=True)
    return {{
        "answer": marker.upper(),
        "cost": "host-work",
        "held": len(held),
        "first": descending[0],
    }}
"#
    )
}

/// An agent that asks to wait for `seconds` before answering: a wait is the run's OWN doing,
/// so it is charged to the invocation's own span.
fn waiting_agent(seconds: u64) -> String {
    format!(
        r#"
import time


def answer(payload):
    time.sleep({seconds})
    return {{"answer": payload["prompt"].upper(), "cost": "wait"}}
"#
    )
}

/// Deliver one invocation of `source` to the deployed-agent run path, exactly as the
/// operation does.
async fn invoke(
    source: &str,
    marker: &str,
) -> Result<agentcore::state::AgentAnswer, agentcore::state::AgentRunFailure> {
    let modules = vec![PackageModule {
        path: ENTRY_FILE.to_string(),
        source: source.to_string(),
    }];
    let entry_point = vec![ENTRY_FILE.to_string(), "answer".to_string()];
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

/// What an invocation answers is what the deployed code produced, however much of THIS
/// host's own time the run cost.
#[tokio::test]
async fn an_invocation_answers_what_the_deployed_code_produced_whatever_host_work_it_cost() {
    // (case, the deployed code, the marker this row sends, the cost the code reports)
    let cases: &[(&str, String, &str, &str)] = &[
        (
            "an agent that answers from its entry point",
            answering_agent(),
            "plain-marker",
            "entrypoint",
        ),
        (
            "an agent that spends host work in native calls first",
            host_working_agent(HOST_WORK_COPIES, HOST_WORK_SORTED_ITEMS),
            "host-work-marker",
            "host-work",
        ),
    ];
    for (case, source, marker, cost) in cases {
        let answered = invoke(source, marker).await;
        let answer = match answered {
            Ok(answer) => answer,
            Err(failure) => panic!(
                "case {case:?}: expected the deployed code's own answer for {marker:?}, got \
                 {failure:?}"
            ),
        };
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected the status the deployed code answered, got {:?} with {:?}",
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
            document["cost"].as_str(),
            Some(*cost),
            "case {case:?}: expected the answer of the agent this row deployed, got {document:?}"
        );
        if *cost == "host-work" {
            // What the native calls came to, which only a run that really performed them can
            // report: the length of the string built out of this row's own marker, and the
            // head of the list it sorted.
            let held = marker.len() * HOST_WORK_COPIES;
            assert_eq!(
                document["held"].as_u64(),
                u64::try_from(held).ok(),
                "case {case:?}: expected the string the run built to be {held} long, got \
                 {document:?}"
            );
            assert_eq!(
                document["first"].as_u64(),
                u64::try_from(HOST_WORK_SORTED_ITEMS - 1).ok(),
                "case {case:?}: expected the head of the list the run sorted, got {document:?}"
            );
        }
    }
}

/// The other side of the rule: the invocation's span IS spent by what the run asks for, so an
/// agent that asks to wait longer than that span runs out of it and is stopped on it — worded
/// on the span the agent ran past, and reached without this host waiting any of it out.
#[tokio::test]
async fn an_agent_that_waits_past_the_invocations_own_span_is_stopped_on_that_span() {
    // (case, how long the agent asks to wait for, in seconds)
    let cases: &[(&str, u64)] = &[
        (
            "an agent that asks to wait just past the invocation's own span",
            agentcore::state::AGENT_RUN_WALL_CLOCK.as_secs() + 1,
        ),
        (
            "an agent that asks to wait far longer than any invocation is given",
            agentcore::state::AGENT_RUN_WALL_CLOCK.as_secs() * 60,
        ),
    ];
    for (case, asked) in cases {
        let answered = invoke(&waiting_agent(*asked), "oversleeping-marker").await;
        match answered {
            Err(agentcore::state::AgentRunFailure::Stopped(reason)) => assert!(
                reason.contains("did not answer within"),
                "case {case:?}: expected the stop to be worded on the span the agent ran past, \
                 got {reason:?}"
            ),
            other => panic!(
                "case {case:?}: expected the invocation to be stopped on its own span, got \
                 {other:?}"
            ),
        }
    }
}
