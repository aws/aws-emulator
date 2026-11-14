// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-role sequence a client scripts around a deployed stage's EXECUTION logging,
//! driven end to end over the `AWS_PROXY` data plane.
//!
//! The rule under test, one level above the recorded case:
//!
//! > Every call of this sequence is a synchronous control-plane call or one request to a
//! > deployed stage, so each reaches a terminal status line INSIDE the request and nothing in
//! > it is a poll. API Gateway writes a method's execution log under the account-level
//! > CloudWatch Logs role, so while the account singleton carries none, a stage patch that
//! > turns execution logging on for any method scope is refused `BadRequestException`
//! > (HTTP 400) with the fixed sentence of that error class and writes nothing; an
//! > `UpdateAccount` naming a role this account's IAM holds that admits
//! > `apigateway.amazonaws.com` is accepted on the FIRST call; the same stage patch is then
//! > accepted and reads back through `GetStage`; the stage answers a request to its
//! > `AWS_PROXY` method by invoking the function, on the first request; and the group API
//! > Gateway publishes for that stage, `API-Gateway-Execution-Logs_{api-id}/{stage}`, holds
//! > the trace of exactly those requests the setting in force for the method they reached
//! > writes one for.
//!
//! Four services have to agree for the rule to hold — API Gateway holds the account, the api
//! and the stage, IAM holds the role the account names, Lambda runs the integration, and
//! CloudWatch Logs holds the group the trace lands in — so all four are mounted on ONE
//! engine, as `core/router/src/lib.rs` mounts them, every role is created through IAM's own
//! `CreateRole` and every delivery is read back through CloudWatch Logs' own
//! `FilterLogEvents`. The wire harness is this crate's existing one: the control plane and
//! the IAM role through [`crate::rest_api_lifecycle`] and [`crate::account_logging_role`],
//! the Lambda calls through [`crate::proxy_binary_media_types`], and the stage invoke and
//! the log read through [`crate::stage_access_logging`].
//!
//! The rows differ along what the rule generalises over: the setting and the method SCOPE
//! that turns execution logging on (the stage-wide wildcard at `INFO`, that wildcard with
//! the data trace beside it, ONE named method at `INFO`, and the wildcard at `ERROR`), the
//! VERB of the request the stage serves, whether the trace carries the payloads of the call
//! or their sizes, and whether a request that reached NO method of the deployment is traced —
//! which the scope and the level together decide.
//!
//! Every call is driven under a generous wall clock, so a step that never answers fails its
//! case instead of hanging the target; nothing here asserts on how long a call took.

use std::future::Future;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};

use crate::account_logging_role::{
    create_role, replace_role, reported_role, role_arn, trusting, BAD_REQUEST,
};
use crate::proxy_binary_media_types::{
    lambda, member, package, wait_until_active, FUNCTION_TIMEOUT, HANDLER, REGION, ROLE,
};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, gateway_with_iam_logs_and_lambda, resource_under, Gateway,
};
use crate::stage_access_logging::{delivered, invoke, traced_line};
use crate::stage_lifecycle::get_stage;

/// How long ONE call of the sequence is given to answer before the case fails.
///
/// Generous: every hop is in-process, so the bound is here to turn "never answers" — the
/// failure mode this sequence had — into a failed assertion rather than a hung test target.
/// No case asserts on the elapsed time, and the runtime is MULTI-THREADED so a stall that
/// blocks its thread rather than awaiting still lets the timer fire.
const ANSWERS_WITHIN: Duration = Duration::from_secs(60);

/// The stage the api is deployed to, and the one resource a request reaches.
const STAGE: &str = "prod";
const RESOURCE: &str = "echo";

/// A path the deployment carries no resource for, which is how a request that reached no
/// method of the stage is driven.
const ABSENT_PATH: &str = "absent";

/// The function every row's `AWS_PROXY` integration names. Each row stands its own emulator
/// up, so one name serves them all.
const FUNCTION: &str = "stage-logging-sequence";

/// The name of the IAM role each row's account patch names, created through IAM's own
/// `CreateRole`, and the path it is created under — the path AWS's own managed push-to-logs
/// role is created under.
const CLOUDWATCH_ROLE: &str = "apigw-push-to-cloudwatch-logs";
const CLOUDWATCH_ROLE_PATH: &str = "/service-role/";

/// The JSON Pointer of the account member that carries the CloudWatch Logs role.
const ROLE_POINTER: &str = "/cloudwatchRoleArn";

/// The message API Gateway refuses a stage configuration that turns execution logging on
/// while the account carries no CloudWatch Logs role with: entirely fixed text for this error
/// class, naming neither the stage, the method scope nor the setting the patch carried.
const ROLE_REQUIRED: &str =
    "CloudWatch Logs role ARN must be set in account settings to enable logging";

/// The handler behind the method: it answers the proxy response envelope with a body naming
/// the verb it was invoked under and the query parameter it was handed.
const HANDLER_SOURCE: &str = r#"
import json


def lambda_handler(event, context):
    query = event.get("queryStringParameters") or {}
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps({"method": event.get("httpMethod"), "id": query.get("id") or "none"}),
    }
"#;

/// One row: the configuration under test and what it makes observable.
struct Row {
    case: &'static str,
    /// The `patchOperations` that turn execution logging on for this row's scope.
    operations: Vec<Value>,
    /// What the accepted patch must make readable: the `methodSettings` KEY the path
    /// addressed, the `MethodSetting` member it writes, and that member's value.
    expected: Vec<(&'static str, &'static str, Value)>,
    /// The verb the request to the deployed stage carries.
    verb: &'static str,
    /// Whether the request the stage SERVED leaves its own trace in the execution group.
    traces_the_served_request: bool,
    /// Whether that trace carries the PAYLOADS of the call rather than their sizes.
    payloads: bool,
    /// Whether a request that reached no method of the deployment leaves a trace.
    traces_an_unmatched_request: bool,
}

/// One `replace` of `path` with `value`.
fn replace(path: &str, value: &str) -> Value {
    json!({ "op": "replace", "path": path, "value": value })
}

/// The rows.
///
/// Source for the spellings: `MethodSetting$loggingLevel` is documented "valid values are
/// OFF, ERROR, and INFO" and `MethodSetting$dataTrace` "specifies whether data trace logging
/// is enabled for this method" (`smithy_query(query="shape", shape="MethodSetting")`); the
/// grouped patch paths (`/{method_setting_key}/logging/loglevel`, `/logging/dataTrace`) and
/// the `*/*` whole-stage key are API Gateway's published method-setting paths, which
/// `Stage$methodSettings` documents as `{resource-path}/{HTTP-METHOD}` or `*/*`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "every method of the stage at INFO, serving a GET",
            operations: vec![replace("/*/*/logging/loglevel", "INFO")],
            expected: vec![("*/*", "loggingLevel", json!("INFO"))],
            verb: "GET",
            traces_the_served_request: true,
            payloads: false,
            traces_an_unmatched_request: true,
        },
        Row {
            case: "every method of the stage at INFO with the data trace, serving a POST",
            operations: vec![
                replace("/*/*/logging/loglevel", "INFO"),
                replace("/*/*/logging/dataTrace", "true"),
            ],
            expected: vec![
                ("*/*", "loggingLevel", json!("INFO")),
                ("*/*", "dataTraceEnabled", json!(true)),
            ],
            verb: "POST",
            traces_the_served_request: true,
            payloads: true,
            traces_an_unmatched_request: true,
        },
        Row {
            case: "one named method at INFO, serving that method's GET",
            operations: vec![replace("/~1echo/GET/logging/loglevel", "INFO")],
            expected: vec![("~1echo/GET", "loggingLevel", json!("INFO"))],
            verb: "GET",
            traces_the_served_request: true,
            payloads: false,
            // The setting is in force for THAT method only, and a request that reached no
            // method of the deployment is covered by no whole-stage entry here.
            traces_an_unmatched_request: false,
        },
        Row {
            case: "every method of the stage at ERROR, serving a GET",
            operations: vec![replace("/*/*/logging/loglevel", "ERROR")],
            expected: vec![("*/*", "loggingLevel", json!("ERROR"))],
            verb: "GET",
            // Logging errors only: a request the stage answered 200 is no error.
            traces_the_served_request: false,
            payloads: false,
            traces_an_unmatched_request: true,
        },
    ]
}

/// Drive one call under [`ANSWERS_WITHIN`], failing the case when it does not answer.
async fn answered<T, F: Future<Output = T>>(case: &str, what: &str, call: F) -> T {
    match tokio::time::timeout(ANSWERS_WITHIN, call).await {
        Ok(answer) => answer,
        Err(_) => panic!(
            "case {case:?}: expected {what} to answer within {ANSWERS_WITHIN:?}, it never \
             reached a status line"
        ),
    }
}

/// `UpdateStage` for this api's stage with `operations`.
async fn update_stage(
    gateway: &Gateway,
    api: &str,
    operations: &[Value],
) -> crate::rest_api_lifecycle::Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{STAGE}"),
            Some(json!({ "patchOperations": operations })),
        )
        .await
}

/// The whole setup a row asserts against: the function, an api with a `GET` and a `POST`
/// method on one resource proxying to it, the grant that admits the invocation, and a
/// deployment released to [`STAGE`]. Answers the api id.
async fn wired(gateway: &Gateway, case: &str, name: &str) -> String {
    let created = answered(
        case,
        "Lambda CreateFunction",
        lambda(
            &gateway.router,
            "POST",
            "/2015-03-31/functions",
            Some(json!({
                "FunctionName": FUNCTION,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": HANDLER,
                "Timeout": FUNCTION_TIMEOUT,
                "Code": {
                    "ZipFile": base64::engine::general_purpose::STANDARD
                        .encode(package(HANDLER_SOURCE)),
                },
            })),
        ),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {} with {:?}",
        created.status,
        created.text()
    );
    let function_arn = member("CreateFunction", &created.json(), "FunctionArn");
    answered(
        case,
        "Lambda reporting the function Active",
        wait_until_active(&gateway.router, FUNCTION),
    )
    .await;

    let (api, root) = create_api(gateway, name).await;
    let resource = resource_under(gateway, &api, &root, RESOURCE).await;
    for verb in ["GET", "POST"] {
        let method = answered(
            case,
            "PutMethod",
            gateway.call(
                "PUT",
                &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
                Some(json!({ "authorizationType": "NONE" })),
            ),
        )
        .await;
        assert_eq!(
            method.status, 201,
            "case {case:?}: expected 201 from PutMethod({verb}), got {} with {:?}",
            method.status, method.body
        );
        let integration = answered(
            case,
            "PutIntegration",
            gateway.call(
                "PUT",
                &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
                Some(json!({
                    "type": "AWS_PROXY",
                    // `integrationHttpMethod` travels under its `jsonName`, which is what
                    // `smithy_query(query="operation", operation="PutIntegration")` reports.
                    "httpMethod": "POST",
                    "uri": format!(
                        "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                         {function_arn}/invocations"
                    ),
                })),
            ),
        )
        .await;
        assert_eq!(
            integration.status, 201,
            "case {case:?}: expected 201 from PutIntegration({verb}), got {} with {:?}",
            integration.status, integration.body
        );
    }

    // "Grants a principal permission to use a function" (`AddPermission`): the grant that
    // admits the invocation API Gateway makes as its own service principal, for every method
    // of every stage of THIS api.
    let granted = answered(
        case,
        "Lambda AddPermission",
        lambda(
            &gateway.router,
            "POST",
            &format!("/2015-03-31/functions/{FUNCTION}/policy"),
            Some(json!({
                "StatementId": "apigateway-invoke",
                "Action": "lambda:InvokeFunction",
                "Principal": "apigateway.amazonaws.com",
                "SourceArn": format!(
                    "arn:aws:execute-api:{REGION}:{account}:{api}/*/*",
                    account = engine::emulator_account_id(),
                ),
            })),
        ),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "case {case:?}: expected HTTP 201 from AddPermission, got {} with {:?}",
        granted.status,
        granted.text()
    );

    let deployment = answered(
        case,
        "CreateDeployment",
        gateway.call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": STAGE })),
        ),
    )
    .await;
    assert_eq!(
        deployment.status, 201,
        "case {case:?}: expected 201 from CreateDeployment, got {} with {:?}",
        deployment.status, deployment.body
    );
    api
}

/// The lines `FilterLogEvents` reports for this stage's execution log group, empty when the
/// account holds no such group at all.
async fn execution_log(gateway: &Gateway, api: &str) -> Vec<String> {
    let group = format!("API-Gateway-Execution-Logs_{api}/{STAGE}");
    delivered(&gateway.router, &group).await.unwrap_or_default()
}

/// RULE: the whole sequence answers call by call, and each of the three outcomes it drives —
/// the refusal without the account's role, the acceptance with it, and the delivery the
/// setting in force asks for — is observable to a client that only reads the API back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_account_role_sequence_answers_every_call_and_makes_each_outcome_observable() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        // One emulator per row, so the account another row configured cannot decide this
        // row's answers.
        let gateway = gateway_with_iam_logs_and_lambda();
        let api = wired(&gateway, case, &format!("logging-sequence-{index}")).await;

        // 1. The account nobody has configured carries no role.
        assert_eq!(
            answered(case, "GetAccount", reported_role(&gateway)).await,
            None,
            "case {case:?}: expected an untouched account to report no {ROLE_POINTER}"
        );

        // 2. So the configuration that turns execution logging on is REFUSED, with the fixed
        //    sentence of this error class.
        let before = get_stage(&gateway, &api, STAGE).await;
        let refused = answered(
            case,
            "the UpdateStage that turns execution logging on with no account role",
            update_stage(&gateway, &api, &row.operations),
        )
        .await;
        assert_failure(&refused, 400, BAD_REQUEST, case);
        assert_eq!(
            refused.body["message"].as_str(),
            Some(ROLE_REQUIRED),
            "case {case:?}: expected the account-role sentence, got {:?}",
            refused.body["message"]
        );

        // 3. And the refused patch left no trace: the stage reads back exactly as it did,
        //    carrying none of the settings the patch named.
        let after = get_stage(&gateway, &api, STAGE).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: expected the refused patch to leave the stage at {:?}, got {:?}",
            before.body, after.body
        );
        for (key, setting, _) in &row.expected {
            let actual = &after.body["methodSettings"][*key][*setting];
            assert_eq!(
                *actual,
                Value::Null,
                "case {case:?}: expected no {setting} under {key} after the refusal, got \
                 {actual:?}"
            );
        }

        // 4. A role this account's IAM holds that admits this service is accepted on the
        //    FIRST call.
        create_role(
            &gateway,
            CLOUDWATCH_ROLE,
            CLOUDWATCH_ROLE_PATH,
            &trusting("apigateway.amazonaws.com"),
        )
        .await;
        let installed = role_arn(CLOUDWATCH_ROLE_PATH, CLOUDWATCH_ROLE);
        let accepted = answered(
            case,
            "the UpdateAccount that installs the role",
            replace_role(&gateway, &installed),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected 200 from UpdateAccount on the first call, got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            answered(case, "GetAccount", reported_role(&gateway))
                .await
                .as_deref(),
            Some(installed.as_str()),
            "case {case:?}: expected GetAccount to report the accepted role"
        );

        // 5. The very same configuration now applies, and reads back through GetStage.
        let applied = answered(
            case,
            "the UpdateStage that turns execution logging on once the role is set",
            update_stage(&gateway, &api, &row.operations),
        )
        .await;
        assert_eq!(
            applied.status, 200,
            "case {case:?}: expected 200 from UpdateStage once the role is set, got {} with {:?}",
            applied.status, applied.body
        );
        let read = get_stage(&gateway, &api, STAGE).await;
        for (key, setting, value) in &row.expected {
            let actual = &read.body["methodSettings"][*key][*setting];
            assert_eq!(
                actual, value,
                "case {case:?}: expected {setting} under {key} to be {value:?}, got {actual:?}"
            );
        }

        // 6. The deployed stage serves the request by invoking the function, first request.
        let served = answered(
            case,
            "the request to the deployed stage",
            invoke(
                &gateway.router,
                &api,
                STAGE,
                row.verb,
                &format!("/{RESOURCE}?id=row-{index}"),
            ),
        )
        .await;
        assert_eq!(
            (served.status, served.json()),
            (200, json!({ "method": row.verb, "id": format!("row-{index}") })),
            "case {case:?}: expected the stage to serve the proxy integration, got {} with {:?}",
            served.status,
            served.text()
        );
        let request_id = served
            .request_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the answer named no request id"));

        // 7. And the group API Gateway publishes for the stage holds the trace of exactly
        //    those requests the setting in force writes one for.
        let events = execution_log(&gateway, &api).await;
        let opened = traced_line(
            &request_id,
            &format!("Execution log for request {request_id}"),
        );
        assert_eq!(
            events.contains(&opened),
            row.traces_the_served_request,
            "case {case:?}: expected the served request to be traced: {}, got {events:?}",
            row.traces_the_served_request
        );
        if row.traces_the_served_request {
            let body_line = if row.payloads {
                traced_line(
                    &request_id,
                    &format!(
                        "Method response body after transformations: {}",
                        served.text()
                    ),
                )
            } else {
                traced_line(
                    &request_id,
                    &format!(
                        "Method response body after transformations: {} byte(s)",
                        served.text().len()
                    ),
                )
            };
            for expected in [
                traced_line(
                    &request_id,
                    &format!("HTTP Method: {}, Resource Path: /{RESOURCE}", row.verb),
                ),
                traced_line(&request_id, "Method completed with status: 200"),
                body_line,
            ] {
                assert!(
                    events.contains(&expected),
                    "case {case:?}: expected the line {expected:?} in the execution log, got \
                     {events:?}"
                );
            }
        }

        // 8. A request that reached no method of the deployment is answered too, and whether
        //    it is traced is what the scope and the level together decide.
        let unmatched = answered(
            case,
            "the request to a path the deployment does not carry",
            invoke(
                &gateway.router,
                &api,
                STAGE,
                "GET",
                &format!("/{ABSENT_PATH}"),
            ),
        )
        .await;
        assert!(
            unmatched.status >= 400,
            "case {case:?}: expected the stage to refuse a path it does not carry, got {} with \
             {:?}",
            unmatched.status,
            unmatched.text()
        );
        let unmatched_id = unmatched
            .request_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the refusal named no request id"));
        let events = execution_log(&gateway, &api).await;
        let opened = traced_line(
            &unmatched_id,
            &format!("Execution log for request {unmatched_id}"),
        );
        assert_eq!(
            events.contains(&opened),
            row.traces_an_unmatched_request,
            "case {case:?}: expected the unmatched request to be traced: {}, got {events:?}",
            row.traces_an_unmatched_request
        );
    }
}
