// SPDX-License-Identifier: CC0-1.0

//! That `UpdateAccount` ANSWERS: the account-CloudWatch-role question is resolved inside the
//! request, so `PATCH /account` reaches a terminal status line on the FIRST call, whatever
//! the role it names.
//!
//! The rule under test, one level above the recorded case:
//!
//! > A control-plane call whose answer depends on another service's records resolves that
//! > question WITHIN the request and answers terminally on the first call — never with a
//! > throttling code and never not at all. For the account's CloudWatch role that means:
//! > `UpdateAccount` replacing `/cloudwatchRoleArn` with a role the account's IAM holds whose
//! > trust policy does not let `apigateway.amazonaws.com` perform `sts:AssumeRole`, or a role
//! > that account's IAM holds nothing under, is answered `BadRequestException` (HTTP 400); a
//! > role whose trust policy does grant that assume is answered HTTP 200 with the role
//! > stored; and because the whole list is judged before anything is written, a refused call
//! > leaves the account reporting EXACTLY the role it held before it — which is what makes
//! > the two answers tellable apart by a client that only reads the account back.
//!
//! Which values earn which of the two answers is tabulated per trust document and per ARN
//! form in the sibling `account_logging_role` suite. What this suite adds is the half that
//! suite cannot see: that every one of those calls TERMINATES (each is driven under a wall
//! clock, so a call that never answers fails the case instead of hanging the target), that
//! the refusal leaves a role the account ALREADY HELD untouched rather than only leaving an
//! empty account empty, and that the whole recorded sequence — refusal, unchanged read,
//! acceptance, and then a deployed stage delivering its execution log under the role that was
//! accepted — runs end to end on one emulator.
//!
//! The rows differ along what the rule generalises over: the role the account HELD when the
//! patch arrived (none, and one already installed), and what IAM answers about the role the
//! patch names (a trust policy admitting this service, one admitting another, one that admits
//! nobody at all, and a role IAM holds nothing under).
//!
//! The roles are created through AWS IAM's OWN `CreateRole` and the log events are read back
//! through CloudWatch Logs' OWN `FilterLogEvents`, all three services on the one engine
//! `core/router/src/lib.rs` mounts them on, so nothing here is asserted against a store this
//! suite wrote by hand. The wire helpers are this crate's own: the control plane and the
//! IAM roles through [`crate::rest_api_lifecycle`] and [`crate::account_logging_role`], the
//! invoke host and the `FilterLogEvents` read through [`crate::stage_access_logging`], so
//! neither call is built twice.

use std::future::Future;
use std::time::Duration;

use serde_json::{json, Value};

use crate::account_logging_role::{
    create_role, replace_role, reported_role, role_arn, trusting, BAD_REQUEST, UNUSABLE_ROLE,
};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, gateway_with_iam_and_logs, resource_under, string_member, Answer,
    Gateway,
};
use crate::stage_access_logging::{delivered, invoke, traced_line};

/// How long a synchronous control-plane call is given to answer before the case fails.
///
/// Generous by two orders of magnitude: the hop this call makes to IAM is in-process, so the
/// answer is immediate — the bound is here to turn "never answers" into a failed assertion
/// rather than a hung test target. Every case runs on a MULTI-THREADED runtime for that
/// reason: a stall that blocks its thread rather than awaiting still lets the timer fire, so a
/// call that holds a lock across the hop fails its case instead of hanging the target.
const ANSWERS_WITHIN: Duration = Duration::from_secs(10);

/// The stage the api below is deployed to, and the one resource a request reaches.
const STAGE: &str = "test";
const RESOURCE: &str = "echo";

/// The method settings that turn the whole execution trace on, as `MethodSetting$loggingLevel`
/// documents its values (`OFF`, `ERROR`, `INFO`).
const LOG_LEVEL_POINTER: &str = "/*/*/logging/loglevel";
const LOG_LEVEL_INFO: &str = "INFO";

/// Drive one call under [`ANSWERS_WITHIN`], failing the case when it does not answer.
async fn answered<F: Future<Output = Answer>>(case: &str, what: &str, call: F) -> Answer {
    match tokio::time::timeout(ANSWERS_WITHIN, call).await {
        Ok(answer) => answer,
        Err(_) => panic!(
            "case {case:?}: expected {what} to answer within {ANSWERS_WITHIN:?}, it never \
             reached a status line"
        ),
    }
}

/// What IAM holds for the role a row's patch names.
enum Held {
    /// `CreateRole` for this `(name, trust document)` before the patch, so IAM answers about
    /// the role the patch then names.
    Role(&'static str, String),
    /// Nothing at all: the patch names a role of this account that was never created.
    Nothing(&'static str),
}

impl Held {
    /// The role name the patch's ARN carries, held or not.
    fn name(&self) -> &'static str {
        match self {
            Self::Role(name, _) | Self::Nothing(name) => name,
        }
    }
}

/// One row: the role the account already held, the role the patch names, and the answer.
struct Row {
    case: &'static str,
    /// The role installed on the account BEFORE the patch under test, or `None` to leave the
    /// account with no CloudWatch role — the two prior states the no-op guarantee spans.
    installed: Option<&'static str>,
    /// What IAM answers about the role the patch names.
    held: Held,
    /// The sentence the patch is refused with, or `None` for a patch that is stored.
    refused_with: Option<&'static str>,
}

/// A trust policy that admits nobody: a readable document carrying no statement at all, so
/// there is no grant for this service to act on.
fn trusting_nobody() -> String {
    json!({ "Version": "2012-10-17", "Statement": [] }).to_string()
}

/// The rows: each prior state crossed with what IAM answers about the named role.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an account with no role, given one that trusts this service",
            installed: None,
            held: Held::Role("apigw-first", trusting("apigateway.amazonaws.com")),
            refused_with: None,
        },
        Row {
            case: "an account with no role, given one that trusts another service",
            installed: None,
            held: Held::Role("lambda-only", trusting("lambda.amazonaws.com")),
            refused_with: Some(UNUSABLE_ROLE),
        },
        Row {
            case: "an installed role, replaced by one that trusts another service",
            installed: Some("apigw-installed"),
            held: Held::Role("lambda-only", trusting("lambda.amazonaws.com")),
            refused_with: Some(UNUSABLE_ROLE),
        },
        Row {
            case: "an installed role, replaced by one IAM holds nothing under",
            installed: Some("apigw-installed"),
            held: Held::Nothing("never-created"),
            refused_with: Some(UNUSABLE_ROLE),
        },
        Row {
            case: "an installed role, replaced by one whose trust policy admits nobody",
            installed: Some("apigw-installed"),
            held: Held::Role("trusts-nobody", trusting_nobody()),
            refused_with: Some(UNUSABLE_ROLE),
        },
        Row {
            case: "an installed role, replaced by another that trusts this service",
            installed: Some("apigw-installed"),
            held: Held::Role("apigw-second", trusting("apigateway.amazonaws.com")),
            refused_with: None,
        },
    ]
}

/// RULE: every `UpdateAccount` naming a CloudWatch role answers on the first call — 400
/// `BadRequestException` for a role API Gateway could not assume, 200 for one it could — and a
/// refused call leaves the account reporting exactly the role it held before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_account_role_patch_always_reaches_a_terminal_answer() {
    for row in rows() {
        // One emulator per row, so what a row stores cannot decide the next row's answer.
        let gateway = gateway_with_iam_and_logs();
        let case = row.case;

        // The prior state: a role this service CAN assume, installed the way a caller does.
        let before = match row.installed {
            None => None,
            Some(name) => {
                create_role(&gateway, name, "/", &trusting("apigateway.amazonaws.com")).await;
                let arn = role_arn("/", name);
                let installed = answered(
                    case,
                    "the UpdateAccount that installs the role",
                    replace_role(&gateway, &arn),
                )
                .await;
                assert_eq!(
                    installed.status, 200,
                    "case {case:?}: expected the prior role to be installed, got {} with {:?}",
                    installed.status, installed.body
                );
                Some(arn)
            }
        };
        assert_eq!(
            answered(case, "GetAccount before the patch", read_account(&gateway))
                .await
                .status,
            200,
            "case {case:?}: expected the account to be readable before the patch"
        );

        if let Held::Role(name, trust) = &row.held {
            create_role(&gateway, name, "/", trust).await;
        }
        let candidate = role_arn("/", row.held.name());
        let answer = answered(
            case,
            "the UpdateAccount under test",
            replace_role(&gateway, &candidate),
        )
        .await;

        match row.refused_with {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected the role to be stored, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    string_member(&answer.body, "cloudwatchRoleArn", case),
                    candidate,
                    "case {case:?}: expected the update to report the role it stored"
                );
                assert_eq!(
                    reported_role(&gateway).await.as_deref(),
                    Some(candidate.as_str()),
                    "case {case:?}: expected GetAccount to report the stored role"
                );
            }
            Some(sentence) => {
                assert_failure(&answer, 400, BAD_REQUEST, case);
                assert_eq!(
                    answer
                        .body
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    sentence,
                    "case {case:?}: expected the refusal {sentence:?}, got {:?}",
                    answer.body
                );
                // The no-op guarantee: the account reports exactly what it held before.
                assert_eq!(
                    reported_role(&gateway).await,
                    before,
                    "case {case:?}: expected the refused patch to leave the account's role as \
                     it was"
                );
            }
        }
    }
}

/// `GetAccount`, as a call whose only assertion is that it answered.
async fn read_account(gateway: &Gateway) -> Answer {
    gateway.call("GET", "/account", None).await
}

/// RULE, end to end: the refusal is terminal and leaves nothing behind, and the role that IS
/// accepted is the one API Gateway then writes the account's logs under — a stage whose method
/// settings carry `loglevel=INFO` delivers the execution log of the request it served into the
/// group API Gateway publishes for that stage.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_refused_role_leaves_no_trace_and_the_accepted_one_carries_the_execution_log() {
    let gateway = gateway_with_iam_and_logs();
    let case = "the recorded sequence";

    // The account starts with no CloudWatch role, which is what an untouched account reports.
    assert_eq!(
        reported_role(&gateway).await,
        None,
        "case {case:?}: expected an untouched account to report no cloudwatchRoleArn"
    );

    // A role IAM holds that trusts another service entirely: refused, on the first call.
    create_role(
        &gateway,
        "lambda-trusted",
        "/",
        &trusting("lambda.amazonaws.com"),
    )
    .await;
    let refused = answered(
        case,
        "UpdateAccount naming the role that does not trust this service",
        replace_role(&gateway, &role_arn("/", "lambda-trusted")),
    )
    .await;
    assert_failure(&refused, 400, BAD_REQUEST, case);
    assert_eq!(
        refused
            .body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        UNUSABLE_ROLE,
        "case {case:?}: expected the refusal {UNUSABLE_ROLE:?}, got {:?}",
        refused.body
    );
    assert_eq!(
        reported_role(&gateway).await,
        None,
        "case {case:?}: expected the refused patch to leave the account without a role"
    );

    // The role that does trust this service: accepted, and read back.
    create_role(
        &gateway,
        "apigw-trusted",
        "/service-role/",
        &trusting("apigateway.amazonaws.com"),
    )
    .await;
    let installed = role_arn("/service-role/", "apigw-trusted");
    let accepted = answered(
        case,
        "UpdateAccount naming the role that trusts this service",
        replace_role(&gateway, &installed),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: expected the trusting role to be stored, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        reported_role(&gateway).await.as_deref(),
        Some(installed.as_str()),
        "case {case:?}: expected GetAccount to report the accepted role"
    );

    // A deployed stage that logs its whole execution trace, under the role just installed.
    let (api, root) = create_api(&gateway, "execution-logged").await;
    let resource = resource_under(&gateway, &api, &root, RESOURCE).await;
    put_mock_method(&gateway, &api, &resource).await;
    deploy(&gateway, &api, STAGE).await;
    let patched = gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{STAGE}"),
            Some(json!({
                "patchOperations": [
                    { "op": "replace", "path": LOG_LEVEL_POINTER, "value": LOG_LEVEL_INFO },
                ],
            })),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "case {case:?}: expected the stage's logging level to be set, got {} with {:?}",
        patched.status, patched.body
    );

    let served = invoke(&gateway.router, &api, STAGE, "GET", &format!("/{RESOURCE}")).await;
    assert_eq!(
        (served.status, served.text()),
        (200, r#"{"at":"echo"}"#.to_string()),
        "case {case:?}: expected the stage to serve the mocked method, got {} with {:?}",
        served.status,
        served.text()
    );
    let request_id = served
        .request_id
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: the answer named no request id"));

    // Read back through CloudWatch Logs' own reader: the group API Gateway publishes for this
    // stage holds the trace of the request that was just served.
    let group = format!("API-Gateway-Execution-Logs_{api}/{STAGE}");
    let events = delivered(&gateway.router, &group).await.unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the stage to have delivered its execution log, but \
             CloudWatch Logs reports no group {group:?} at all"
        )
    });
    assert!(
        !events.is_empty(),
        "case {case:?}: expected the execution log of the served request in {group:?}, got no \
         event at all"
    );
    for expected in [
        traced_line(
            &request_id,
            &format!("Execution log for request {request_id}"),
        ),
        traced_line(
            &request_id,
            &format!("HTTP Method: GET, Resource Path: /{RESOURCE}"),
        ),
        traced_line(&request_id, "Method completed with status: 200"),
    ] {
        assert!(
            events.contains(&expected),
            "case {case:?}: expected the line {expected:?} in {group:?}, got {events:?}"
        );
    }
}

/// A `GET` method with a `MOCK` integration answering one body, which is the integration a
/// stage can serve with no backend at all.
async fn put_mock_method(gateway: &Gateway, api: &str, resource: &str) {
    let method = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        method.status, 201,
        "setup PutMethod: expected 201, got {} with {:?}",
        method.status, method.body
    );
    let integration = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            Some(json!({ "type": "MOCK" })),
        )
        .await;
    assert_eq!(
        integration.status, 201,
        "setup PutIntegration: expected 201, got {} with {:?}",
        integration.status, integration.body
    );
    let response = gateway
        .call(
            "PUT",
            &format!(
                "/restapis/{api}/resources/{resource}/methods/GET/integration/responses/200"
            ),
            Some(json!({
                "responseTemplates": { "application/json": r#"{"at":"echo"}"# },
            })),
        )
        .await;
    assert_eq!(
        response.status, 201,
        "setup PutIntegrationResponse: expected 201, got {} with {:?}",
        response.status, response.body
    );
}

/// `CreateDeployment` into `stage`.
async fn deploy(gateway: &Gateway, api: &str, stage: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": stage })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "setup CreateDeployment({stage}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

