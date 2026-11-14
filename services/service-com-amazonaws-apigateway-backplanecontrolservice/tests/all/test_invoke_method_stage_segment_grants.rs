// SPDX-License-Identifier: CC0-1.0

//! What ONE function's resource policy decides for the two callers of ONE `AWS_PROXY` method:
//! a client of the DEPLOYED stage, and `TestInvokeMethod` simulating the same method.
//!
//! The rule under test, one level above the recorded case:
//!
//! > API Gateway invokes a proxy integration's function as its own service principal under the
//! > ARN of the method being invoked, and that ARN names the STAGE the invocation is
//! > attributed to: the deployed stage's own name for a request served by that stage, and the
//! > stage AWS reserves for a simulated invocation (`test-invoke-stage`) for
//! > `TestInvokeMethod`. So the function's resource-based policy decides the two callers
//! > SEPARATELY — a grant whose `SourceArn` covers one stage segment admits that caller only,
//! > and the caller it does not cover never reaches the function and is answered through the
//! > api's `API_CONFIGURATION_ERROR` gateway response (`500` with
//! > `{"message":"Internal server error"}`). `TestInvokeMethod` reports that refusal in its own
//! > response envelope — it answers `200` carrying `status`, `body`, `headers`,
//! > `multiValueHeaders`, `log` and `latency` whatever the function's policy decides, which is
//! > how a caller tells the permission-scoped refusal from a success.
//!
//! The rows differ along what the rule generalises over: WHICH stage segments the function's
//! grants cover (none at all, the deployed stage only, the reserved test-invoke stage only,
//! both as two statements, every stage through a wildcard, and the deployed stage under
//! another verb) — the axis each caller's decision turns on — and so also along HOW MANY
//! statements the policy holds (zero, one, two).
//!
//! Each row asserts BOTH callers of the same wired method, and then how many RUNS of the
//! function Amazon CloudWatch Logs holds: a run is one `START RequestId:` record in the
//! function's own group, so a row expecting one caller admitted and the other refused is
//! evidence that exactly one invocation happened. A status alone cannot show that.
//!
//! Only a cross-service suite can pin this: the api, its stage and the simulation are API
//! Gateway's, the function and its policy are AWS Lambda's, and the log of a run that HAPPENED
//! is CloudWatch Logs' — all three over ONE engine. The harness that wires them is
//! [`crate::test_invoke_method_lambda_permission`]'s, reused here rather than written twice;
//! that suite varies the grant for the SIMULATED caller alone, where this one holds the two
//! callers against each other.
//!
//! What a deployed `AWS_IAM` method makes of the CREDENTIAL a request carries is a different
//! axis, pinned where the emulator really verifies signatures
//! (`crate::stage_invoke_signed`, `core/router/tests/all/apigateway_iam_authenticated_invoke.rs`),
//! so the method here authenticates nobody and every row observes the resource-policy
//! decision alone.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::test_invoke_method_lambda_permission::{
    accepted, dispatch, emulator, grant, runs_logged, wired, Answered, INTERNAL_SERVER_ERROR,
    INVALID_PERMISSIONS, MARKER, REGION, RESOURCE_PATH, STAGE, TEST_INVOKE_STAGE,
};

/// The signing name a request to a deployed stage's invoke URL carries: the `execute-api`
/// service AWS publishes the invoke endpoint under, not this service's control-plane name.
const INVOKE_SIGNING_NAME: &str = "execute-api";

/// The members `TestInvokeMethod`'s response envelope carries, from
/// `smithy_query(query="operation",operation="TestInvokeMethod")`: `status` (Integer), `body`
/// (String), `headers`, `multiValueHeaders`, `log` (String) and `latency` (Long).
const ENVELOPE_MEMBERS: &[&str] = &[
    "status",
    "body",
    "headers",
    "multiValueHeaders",
    "log",
    "latency",
];

/// The sentence the execution log of an invocation that was MADE carries.
const COMPLETED: &str = "Successfully completed execution";

/// One row: which `SourceArn`s the function's grants cover, and which of the two callers is
/// then admitted to the function.
struct Row {
    case: &'static str,
    /// The resource half of each grant's `SourceArn`, under the api's own id. `{api}` stands
    /// for the api under test, `{stage}` for the stage it is deployed to, `{test-invoke-stage}`
    /// for the segment AWS reserves for a simulated invocation and `{path}` for the resource
    /// path its method sits on; an empty slice is a function carrying no grant at all.
    sources: &'static [&'static str],
    /// Whether a client of the DEPLOYED stage reaches the function.
    deployed_invoked: bool,
    /// Whether `TestInvokeMethod` reaches the function.
    simulated_invoked: bool,
}

/// The rows: every way the grants of one function cover — or fail to cover — the deployed
/// stage's own invocation and the simulated one.
const ROWS: &[Row] = &[
    Row {
        case: "no grant at all",
        sources: &[],
        deployed_invoked: false,
        simulated_invoked: false,
    },
    Row {
        case: "one grant naming the deployed stage only",
        sources: &["{api}/{stage}/GET/{path}"],
        deployed_invoked: true,
        simulated_invoked: false,
    },
    Row {
        case: "one grant naming the reserved test-invoke stage only",
        sources: &["{api}/{test-invoke-stage}/GET/{path}"],
        deployed_invoked: false,
        simulated_invoked: true,
    },
    Row {
        case: "two grants, one naming each stage segment",
        sources: &[
            "{api}/{stage}/GET/{path}",
            "{api}/{test-invoke-stage}/GET/{path}",
        ],
        deployed_invoked: true,
        simulated_invoked: true,
    },
    Row {
        case: "one grant covering every stage of the api",
        sources: &["{api}/*/*/{path}"],
        deployed_invoked: true,
        simulated_invoked: true,
    },
    Row {
        case: "one grant naming the deployed stage under another verb",
        sources: &["{api}/{stage}/POST/{path}"],
        deployed_invoked: false,
        simulated_invoked: false,
    },
];

/// What a client of the api's published invoke URL is served for the deployed method.
async fn invoke_deployed_stage(router: &MultiServiceRouter, api: &str) -> Answered {
    dispatch(
        router,
        "GET",
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}/{RESOURCE_PATH}"),
        INVOKE_SIGNING_NAME,
        &[],
        Bytes::new(),
    )
    .await
}

/// The `status` and the `body` one answered call reports, whichever caller made it.
fn reported(case: &str, caller: &str, status: i64, body: &str) -> (i64, Option<Value>) {
    let parsed = serde_json::from_str::<Value>(body).ok();
    assert!(
        parsed.is_some(),
        "case {case:?}: expected the {caller} caller's body to be the JSON either the handler \
         or the gateway response answers, got {body:?} with status {status}"
    );
    (status, parsed)
}

#[tokio::test]
async fn one_functions_policy_decides_the_deployed_stage_and_the_simulated_caller_separately() {
    for row in ROWS {
        let case = row.case;
        // One emulator per row, so the policy, the api and the runs a row logs are its own.
        let router = emulator();
        let (api, resource) = wired(&router).await;
        for (index, source) in row.sources.iter().enumerate() {
            let source_arn = format!(
                "arn:aws:execute-api:{REGION}:{account}:{resource_half}",
                account = engine::emulator_account_id(),
                resource_half = source
                    .replace("{api}", &api)
                    .replace("{stage}", STAGE)
                    .replace("{test-invoke-stage}", TEST_INVOKE_STAGE)
                    .replace("{path}", RESOURCE_PATH),
            );
            grant(
                &router,
                case,
                &format!("apigateway-invoke-{index}"),
                &source_arn,
            )
            .await;
        }

        // 1. The deployed stage's own caller: the handler's answer, or the api's
        //    configuration-error gateway response.
        let served = invoke_deployed_stage(&router, &api).await;
        let deployed = reported(
            case,
            "deployed-stage",
            i64::from(served.status),
            &served.text(),
        );
        if row.deployed_invoked {
            assert_eq!(
                deployed,
                (200, Some(json!({ "marker": MARKER }))),
                "case {case:?}: expected the deployed stage to serve the handler's own 200 and \
                 body, got {} with {:?}",
                served.status,
                served.text()
            );
        } else {
            assert_eq!(
                deployed,
                (500, Some(json!({ "message": INTERNAL_SERVER_ERROR }))),
                "case {case:?}: expected the deployed stage to answer the api's \
                 configuration-error refusal, got {} with {:?}",
                served.status,
                served.text()
            );
        }

        // 2. The simulated caller: always the envelope, never a refusal of the control-plane
        //    call itself.
        let simulated = accepted(
            &router,
            "TestInvokeMethod",
            "POST",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(json!({ "pathWithQueryString": format!("/{RESOURCE_PATH}") })),
            200,
        )
        .await;
        for member in ENVELOPE_MEMBERS {
            let carried = simulated.get(*member);
            assert!(
                carried.is_some_and(|value| !value.is_null()),
                "case {case:?}: expected the response envelope to carry {member:?}, got \
                 {simulated}"
            );
        }
        let latency = simulated.get("latency").and_then(Value::as_i64);
        assert!(
            latency.is_some_and(|latency| latency >= 0),
            "case {case:?}: expected the envelope's latency to be a whole number of \
             milliseconds, got {latency:?}"
        );
        let status = simulated
            .get("status")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: no numeric status in {simulated}"));
        let body = simulated
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let log = simulated
            .get("log")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let test_invoked = reported(case, "test-invoke", status, &body);
        if row.simulated_invoked {
            assert_eq!(
                test_invoked,
                (200, Some(json!({ "marker": MARKER }))),
                "case {case:?}: expected the simulation to report the handler's own 200 and \
                 body, got {status} with {body:?}"
            );
            assert!(
                log.contains(COMPLETED),
                "case {case:?}: expected the execution log to report the invocation, got {log:?}"
            );
        } else {
            assert_eq!(
                test_invoked,
                (500, Some(json!({ "message": INTERNAL_SERVER_ERROR }))),
                "case {case:?}: expected the simulation to report the api's \
                 configuration-error refusal, got {status} with {body:?}"
            );
            assert!(
                log.contains(INVALID_PERMISSIONS),
                "case {case:?}: expected the execution log to name the configuration error \
                 {INVALID_PERMISSIONS:?}, got {log:?}"
            );
        }

        // 3. What a status alone cannot show: how many invocations of the function really
        //    happened — one run's own log per admitted caller, and none for a refused one. The
        //    run's records are written by the invocation itself, so this is read once rather
        //    than waited for.
        let expected = usize::from(row.deployed_invoked) + usize::from(row.simulated_invoked);
        let runs = runs_logged(&router).await;
        assert_eq!(
            runs, expected,
            "case {case:?}: expected {expected} run(s) of the function logged, got {runs}"
        );
    }
}
