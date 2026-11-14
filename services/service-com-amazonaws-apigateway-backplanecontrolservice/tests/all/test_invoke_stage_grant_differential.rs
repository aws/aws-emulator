// SPDX-License-Identifier: CC0-1.0

//! What the METHOD's own authorization does to the resource-policy decision a simulated
//! invocation is subject to, and what a grant ADDED to the function's policy changes.
//!
//! The rule under test, one level above the recorded case:
//!
//! > A simulated invocation (`TestInvokeMethod`) reaches the method's integration whatever the
//! > method's own `authorizationType` or API key gate says — those gate the DEPLOYED caller —
//! > and the one thing that decides whether the integration's function is invoked is that
//! > FUNCTION's resource-based policy, judged against the ARN of the method under the stage AWS
//! > reserves for a test invocation (`test-invoke-stage`). That decision is read from the
//! > policy as it stands at each call: a grant added to a function that admitted the simulation
//! > nothing turns the next simulation into the handler's own answer, and leaves every caller
//! > the policy already admitted answering exactly as before.
//!
//! The rows differ along what the rule generalises over: the gate the method declares for its
//! deployed caller — `Method$authorizationType` `NONE` and `AWS_IAM`, and
//! `Method$apiKeyRequired` — each driven through a function whose only grant names the DEPLOYED
//! stage and then through the same function once a grant for the reserved stage is added beside
//! it.
//!
//! Only a cross-service suite can pin this: the api, its stage and the simulation are API
//! Gateway's, the function and its policy are AWS Lambda's, and the log of a run that HAPPENED
//! is CloudWatch Logs' — all three over ONE engine. The harness that wires them is
//! [`crate::test_invoke_method_lambda_permission`]'s, reused here rather than written twice.
//!
//! Where this sits beside its siblings: [`crate::test_invoke_method_lambda_permission`] varies
//! the grant for the simulated caller alone,
//! [`crate::test_invoke_method_stage_segment_grants`] holds the two callers of one method
//! against each other over the stage segments the grants name, and
//! [`crate::test_invoke_method_data_plane_parity`] pins that the simulation applies neither the
//! `authorizationType` nor the API key gate. What is added here is the CROSSING of the method's
//! gate with the function's policy, and the SEQUENCE a client follows when a test invoke is
//! refused: add the grant for the test-invoke source ARN and call again.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::test_invoke_method_lambda_permission::{
    accepted, dispatch, emulator, grant, lambda, runs_logged, wired, Answered, FUNCTION,
    INTERNAL_SERVER_ERROR, INVALID_PERMISSIONS, MARKER, REGION, RESOURCE_PATH, STAGE,
    TEST_INVOKE_STAGE,
};

/// The signing name a request to a deployed stage's invoke URL carries: the `execute-api`
/// service AWS publishes the invoke endpoint under, not this service's control-plane name.
const INVOKE_SIGNING_NAME: &str = "execute-api";

/// The verb every method below is put on, and so the verb segment of every grant.
const VERB: &str = "GET";

/// The sentence the execution log of an invocation that was MADE carries.
const COMPLETED: &str = "Successfully completed execution";

/// The api's root resource, which the methods this suite adds are created under.
async fn root_resource(router: &MultiServiceRouter, api: &str) -> String {
    let resources = accepted(
        router,
        "GetResources",
        "GET",
        &format!("/restapis/{api}/resources"),
        None,
        200,
    )
    .await;
    resources
        .get("item")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("path").and_then(Value::as_str) == Some("/"))
                .or_else(|| items.first())
        })
        .and_then(|item| item.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("setup GetResources: no root resource in {resources}"))
}

/// The ARN Lambda itself reports for the function the harness created, which is what an
/// integration `uri` names — taken from the service that owns the function rather than written
/// here.
async fn function_arn(router: &MultiServiceRouter) -> String {
    let (status, configuration) = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup GetFunctionConfiguration: expected HTTP 200, got {status} with {configuration}"
    );
    configuration
        .get("FunctionArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("setup GetFunctionConfiguration: no FunctionArn in {configuration}")
        })
        .to_string()
}

/// One more method of the api under `root`, on its own resource path, carrying `gate` as the
/// `PutMethod` request and the same proxy integration to the harness's function. Answers the
/// resource id the method sits on.
async fn method_under(
    router: &MultiServiceRouter,
    api: &str,
    root: &str,
    path_part: &str,
    gate: Value,
    function: &str,
) -> String {
    let resource = accepted(
        router,
        "CreateResource",
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": path_part })),
        201,
    )
    .await;
    let resource = resource
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup CreateResource({path_part}): no id in {resource}"))
        .to_string();
    accepted(
        router,
        "PutMethod",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/{VERB}"),
        Some(gate),
        201,
    )
    .await;
    accepted(
        router,
        "PutIntegration",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/{VERB}/integration"),
        Some(json!({
            "type": "AWS_PROXY",
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                 {function}/invocations"
            ),
        })),
        201,
    )
    .await;
    resource
}

/// The `SourceArn` a grant to `apigateway.amazonaws.com` is narrowed to: the method of this
/// api, under `stage` as its stage segment.
fn method_arn(api: &str, stage: &str, path_part: &str) -> String {
    format!(
        "arn:aws:execute-api:{REGION}:{account}:{api}/{stage}/{VERB}/{path_part}",
        account = engine::emulator_account_id(),
    )
}

/// One simulated invocation of the method on `resource`, through the operation's own envelope:
/// the simulated `status`, the simulated `body` and the execution `log`.
async fn simulate(
    router: &MultiServiceRouter,
    case: &str,
    api: &str,
    resource: &str,
    path_part: &str,
) -> (i64, Option<Value>, String) {
    let simulated = accepted(
        router,
        &format!("TestInvokeMethod({case})"),
        "POST",
        &format!("/restapis/{api}/resources/{resource}/methods/{VERB}"),
        Some(json!({ "pathWithQueryString": format!("/{path_part}") })),
        200,
    )
    .await;
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
    (status, serde_json::from_str::<Value>(&body).ok(), log)
}

/// What a client of the api's published invoke URL is served for the deployed method — the one
/// request a caller of the DEPLOYED stage makes, through the harness's own wire helper.
async fn invoke_deployed_stage(router: &MultiServiceRouter, api: &str) -> Answered {
    dispatch(
        router,
        VERB,
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}/{RESOURCE_PATH}"),
        INVOKE_SIGNING_NAME,
        &[],
        Bytes::new(),
    )
    .await
}

/// One row: the gate the method declares for its DEPLOYED caller, which decides nothing about
/// the simulation.
struct Row {
    case: &'static str,
    /// The resource path the method sits on, which is also the path segment of its grants.
    path_part: &'static str,
    /// The `PutMethod` request: the `Method$authorizationType` and, where the row carries it,
    /// `Method$apiKeyRequired` — the two gates the data plane applies to a deployed caller
    /// (`smithy_query(query="shape",shape="Method")`).
    gate: &'static str,
}

/// The rows: each gate a method may declare without a second record to name.
const ROWS: &[Row] = &[
    Row {
        case: "a method that authenticates nobody",
        path_part: "open",
        gate: r#"{"authorizationType":"NONE"}"#,
    },
    Row {
        case: "a method that authenticates its deployed caller",
        path_part: "signed",
        gate: r#"{"authorizationType":"AWS_IAM"}"#,
    },
    Row {
        case: "a method requiring an API key of its deployed caller",
        path_part: "keyed",
        gate: r#"{"authorizationType":"NONE","apiKeyRequired":true}"#,
    },
];

/// Whatever gate the method declares for its deployed caller, the simulation is decided by the
/// FUNCTION's policy alone: refused while its only grant names the deployed stage, and served
/// once a grant names the stage reserved for a test invocation.
#[tokio::test]
async fn the_methods_own_gate_decides_nothing_about_the_simulated_invocation() {
    let router = emulator();
    let (api, _) = wired(&router).await;
    let root = root_resource(&router, &api).await;
    let function = function_arn(&router).await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let gate: Value = serde_json::from_str(row.gate).expect("the row's PutMethod request");
        let resource = method_under(&router, &api, &root, row.path_part, gate, &function).await;

        // The function admits API Gateway this method under the DEPLOYED stage only, which is
        // not the stage segment a simulated invocation is judged under.
        grant(
            &router,
            case,
            &format!("deployed-{index}"),
            &method_arn(&api, STAGE, row.path_part),
        )
        .await;
        let runs_before = runs_logged(&router).await;
        let (status, body, log) = simulate(&router, case, &api, &resource, row.path_part).await;
        assert_eq!(
            (status, body),
            (500, Some(json!({ "message": INTERNAL_SERVER_ERROR }))),
            "case {case:?}: expected the simulation to report the api's configuration-error \
             refusal while the only grant names the deployed stage, got {status} with {log:?}"
        );
        assert!(
            log.contains(INVALID_PERMISSIONS),
            "case {case:?}: expected the execution log to name the configuration error \
             {INVALID_PERMISSIONS:?}, got {log:?}"
        );
        let runs_after = runs_logged(&router).await;
        assert_eq!(
            runs_after, runs_before,
            "case {case:?}: expected no run of the function to be logged by a refused \
             simulation, got {} more",
            runs_after - runs_before
        );

        // The grant for the reserved stage, added beside it: the SAME simulation is now the
        // handler's own answer, and one run of the function is logged for it.
        grant(
            &router,
            case,
            &format!("test-invoke-{index}"),
            &method_arn(&api, TEST_INVOKE_STAGE, row.path_part),
        )
        .await;
        let (status, body, log) = simulate(&router, case, &api, &resource, row.path_part).await;
        assert_eq!(
            (status, body),
            (200, Some(json!({ "marker": MARKER }))),
            "case {case:?}: expected the simulation to report the handler's own answer once the \
             reserved stage is granted, got {status} with {log:?}"
        );
        assert!(
            log.contains(COMPLETED),
            "case {case:?}: expected the execution log to report the invocation, got {log:?}"
        );
        let runs_served = runs_logged(&router).await;
        assert_eq!(
            runs_served,
            runs_after + 1,
            "case {case:?}: expected exactly one run of the function logged by the served \
             simulation, got {} more",
            runs_served - runs_after
        );
    }
}

/// The SEQUENCE a client follows when a test invoke is refused: the grant it adds for the
/// test-invoke source ARN admits the next simulation, and the caller the policy already
/// admitted — the deployed stage — answers exactly as it did before.
#[tokio::test]
async fn a_grant_added_for_the_reserved_stage_admits_the_next_simulation_and_nothing_else() {
    let router = emulator();
    let (api, resource) = wired(&router).await;

    let case = "the deployed stage's grant alone";
    grant(
        &router,
        case,
        "deployed",
        &method_arn(&api, STAGE, RESOURCE_PATH),
    )
    .await;

    // The deployed caller, served by the grant it is covered by.
    let served = invoke_deployed_stage(&router, &api).await;
    assert_eq!(
        (served.status, served.json()),
        (200, json!({ "marker": MARKER })),
        "case {case:?}: expected the deployed stage to serve the handler's own answer, got {} \
         with {:?}",
        served.status,
        served.text()
    );
    // The simulated caller, refused under the stage segment no grant covers.
    let (status, body, _) = simulate(&router, case, &api, &resource, RESOURCE_PATH).await;
    assert_eq!(
        (status, body),
        (500, Some(json!({ "message": INTERNAL_SERVER_ERROR }))),
        "case {case:?}: expected the simulation to be refused by the function's policy, got \
         {status}"
    );
    let runs_before = runs_logged(&router).await;
    assert_eq!(
        runs_before, 1,
        "case {case:?}: expected the deployed caller's run alone to be logged, got {runs_before}"
    );

    let case = "the reserved stage's grant added beside it";
    grant(
        &router,
        case,
        "test-invoke",
        &method_arn(&api, TEST_INVOKE_STAGE, RESOURCE_PATH),
    )
    .await;

    let (status, body, log) = simulate(&router, case, &api, &resource, RESOURCE_PATH).await;
    assert_eq!(
        (status, body),
        (200, Some(json!({ "marker": MARKER }))),
        "case {case:?}: expected the same simulation to report the handler's own answer, got \
         {status} with {log:?}"
    );
    // And the caller the policy already admitted is unchanged: the same request, the same
    // answer, with its own run logged beside the simulation's.
    let served_again = invoke_deployed_stage(&router, &api).await;
    assert_eq!(
        (served_again.status, served_again.json()),
        (served.status, served.json()),
        "case {case:?}: expected the deployed stage's answer to be what it was before the grant \
         ({} with {:?}), got {} with {:?}",
        served.status,
        served.text(),
        served_again.status,
        served_again.text()
    );
    let runs_after = runs_logged(&router).await;
    assert_eq!(
        runs_after, 3,
        "case {case:?}: expected the two runs made after the grant to be logged beside the \
         first, got {runs_after}"
    );
}
