// SPDX-License-Identifier: CC0-1.0

//! The refusal `StopDeployment` answers for a deployment AppConfig holds no record of.
//!
//! The rule under test, in one sentence: a stop acts on the deployment through the
//! component that owns a deployment in flight, so the refusal for a deployment that
//! component holds no record of reaches the caller RELAYED — the component's own sentence,
//! then the clause naming the call it came from: the component that answered, the refusal's
//! own HTTP status, the error code it is answered under, that call's own request id, and the
//! proxy it came through, of which there is none.
//!
//! Every row drives a real HTTP request through the router the emulator's front door uses,
//! so what is asserted is the `Message` an AppConfig client reads on the wire.
//!
//! The table spans what the rule generalises over rather than the one recorded lookup: the
//! number asked for (one the environment never issued, one a SIBLING environment did issue,
//! `0` below the sequence, one far past it, and the published upper bound of the `Integer`
//! member), which environment of the application is addressed (one that has issued a
//! deployment, one that never has, and one that never existed) and what the request's
//! `Allow-Revert` says — none of which changes the refusal beyond the identifiers it echoes.
//!
//! The second suite pins the BOUNDARY of that rule: a level the control plane resolves
//! itself before any of this is relayed — the application — is refused in AppConfig's own
//! words with no relayed clause, and so is the deployment REPORT, which reads the deployment
//! out of the control-plane tree rather than stopping it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    account, create_application, create_environment, create_profile, host_version, json_call,
    router, send, Answer,
};

/// The component a stop settles a deployment through, as the relayed refusal names it.
const DEPLOYMENT_COMPONENT: &str = "AWSAppConfigData";

/// The clause's last field: there is no proxy in front of the call, which the rendering
/// states rather than leaving out.
const NO_PROXY: &str = "; Proxy: null)";

/// The upper bound of a `DeploymentNumber`, which the model targets at `Integer`
/// (`smithy_query(query="operation",operation="StopDeployment")`).
const MAX_DEPLOYMENT_NUMBER: i64 = i32::MAX as i64;

/// `DELETE /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}`,
/// `StopDeployment`'s modelled route, with `AllowRevert` in the `Allow-Revert` header the
/// model binds it to (`smithy_query(query="operation",operation="StopDeployment")`).
async fn stop(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
    allow_revert: Option<bool>,
) -> Answer {
    let headers: Vec<(&str, String)> = allow_revert
        .map(|allow| vec![("Allow-Revert", allow.to_string())])
        .unwrap_or_default();
    send(
        router,
        "DELETE",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        "application/json",
        Vec::new(),
        &headers,
    )
    .await
}

/// `GET /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}`,
/// `GetDeployment`'s modelled route.
async fn get_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        &Value::Null,
    )
    .await
}

/// A strategy whose rollout lasts the longest the model allows, so a deployment paced by it
/// is still in flight however long the suite around it takes.
async fn in_flight_strategy(router: &MultiServiceRouter, name: &str) -> String {
    let answer = json_call(
        router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": name,
            "DeploymentDurationInMinutes": 1440,
            "GrowthFactor": 50.0,
            "GrowthType": "LINEAR",
            "FinalBakeTimeInMinutes": 0,
            "ReplicateTo": "NONE",
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateDeploymentStrategy must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Id")
        .expect("CreateDeploymentStrategy answers an Id")
}

/// Assert `answer` is the relayed `ResourceNotFoundException` for `sentence`, and answer the
/// request id its clause named.
///
/// The clause's status is compared against the ANSWER's own HTTP status, so the message and
/// the response can never disagree about the status the refusal carries.
fn relayed_request_id(case: &str, answer: &Answer, sentence: &str) -> String {
    assert_eq!(
        answer.status,
        404,
        "case {case:?}: StopDeployment must answer 404 for a deployment nothing holds, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let code = answer.error_code();
    assert!(
        code.contains("ResourceNotFoundException"),
        "case {case:?}: StopDeployment must answer ResourceNotFoundException, got {code:?}"
    );
    let message = answer.string("Message").unwrap_or_default();
    let head = format!(
        "{sentence} (Service: {DEPLOYMENT_COMPONENT}; Status Code: {}; Error Code: \
         ResourceNotFoundException; Request ID: ",
        answer.status
    );
    assert!(
        message.starts_with(&head),
        "case {case:?}: expected the refusal to start {head:?}, got {message:?}"
    );
    assert!(
        message.ends_with(NO_PROXY),
        "case {case:?}: expected the refusal to end {NO_PROXY:?}, got {message:?}"
    );
    let request_id = message
        .strip_prefix(&head)
        .and_then(|rest| rest.strip_suffix(NO_PROXY))
        .unwrap_or_default()
        .to_string();
    assert!(
        ::service_common::query_error_envelope::is_request_id(&request_id),
        "case {case:?}: the clause must name the call by a request id, got {request_id:?} in \
         {message:?}"
    );
    request_id
}

/// Every deployment lookup a stop cannot resolve is refused with the component's sentence
/// and the clause naming the relayed call, and each call is named by its own request id.
#[tokio::test]
async fn a_stop_that_resolves_no_deployment_relays_the_components_refusal() {
    let router = router();
    let account = account();
    let application = create_application(&router, "relayed-stop-misses").await;
    // Two environments: one that has issued a deployment number and one that never has, so
    // the same number is a miss under one and a hit under the other.
    let deployed = create_environment(&router, &application, "deployed-env").await;
    let untouched = create_environment(&router, &application, "untouched-env").await;
    let profile = create_profile(&router, &application, "relayed").await;
    let version = host_version(
        &router,
        &application,
        &profile,
        "relayed stop",
        br#"{"flag":true}"#,
        None,
    )
    .await;
    let strategy = in_flight_strategy(&router, "relayed-stop-in-flight").await;

    let started = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments/{deployed}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": version.to_string(),
        }),
    )
    .await;
    assert_eq!(
        started.status,
        201,
        "StartDeployment must answer 201, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    assert_eq!(
        started.json()["DeploymentNumber"].as_i64(),
        Some(1),
        "an environment's first deployment is number 1, got {}",
        started.json()
    );

    // Rows differ along the number asked for, the environment it is asked under, and what
    // the request says about a revert. The number 1 is a HIT under `deployed` and a miss
    // under `untouched`, which is what makes the lookup per environment rather than per
    // application.
    let rows: Vec<(&str, &str, i64, Option<bool>)> = vec![
        (
            "the next number the environment has not issued",
            &deployed,
            2,
            None,
        ),
        (
            "a number a sibling environment issued, with a revert allowed",
            &untouched,
            1,
            Some(true),
        ),
        ("the number below the sequence", &deployed, 0, Some(false)),
        ("a number far past the sequence", &deployed, 4096, None),
        (
            "the upper bound of the Integer member",
            &untouched,
            MAX_DEPLOYMENT_NUMBER,
            Some(true),
        ),
        (
            "an environment that never existed",
            "nosuchenv",
            1,
            None,
        ),
    ];

    let mut request_ids: Vec<String> = Vec::new();
    for (case, environment, number, allow_revert) in rows {
        let sentence = format!(
            "Deployment with Application Id {application}, Environment Id {environment}, and \
             deploymentNumber {number} could not be found for account {account}."
        );
        let answer = stop(&router, &application, environment, number, allow_revert).await;
        request_ids.push(relayed_request_id(case, &answer, &sentence));
    }

    // The request id belongs to the relayed CALL, so no two refusals name one call — not
    // even two misses on the same deployment number.
    let repeated = stop(&router, &application, &untouched, 1, None).await;
    let sentence = format!(
        "Deployment with Application Id {application}, Environment Id {untouched}, and \
         deploymentNumber 1 could not be found for account {account}."
    );
    request_ids.push(relayed_request_id(
        "the same miss asked for twice",
        &repeated,
        &sentence,
    ));
    let mut distinct = request_ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        request_ids.len(),
        "case \"every relayed call\": each refusal must name its own call, got {request_ids:?}"
    );

    // The number that IS held is still settled, so the rows above pin a miss rather than a
    // stop that refuses everything.
    let held = stop(&router, &application, &deployed, 1, None).await;
    assert_eq!(
        held.status,
        202,
        "the deployment the environment issued must still be stopped, got {} / {}",
        held.status,
        String::from_utf8_lossy(&held.body)
    );
    let state = held.string("State").unwrap_or_default();
    assert!(
        matches!(state.as_str(), "ROLLING_BACK" | "ROLLED_BACK"),
        "the stop of an in-flight deployment must roll it back, got {state:?}"
    );
}

/// The boundary of the rule: a refusal the CONTROL PLANE answers for itself carries no
/// relayed clause.
///
/// Two of them, which is what the rows differ along: the application, the level every
/// operation addressed through an application id resolves before anything else — so a stop
/// never reaches the component with it — and `GetDeployment`, which REPORTS a deployment out
/// of the control-plane tree rather than stopping one.
#[tokio::test]
async fn a_refusal_the_control_plane_answers_itself_carries_no_relayed_clause() {
    let router = router();
    let account = account();
    let application = create_application(&router, "relayed-stop-boundary").await;
    let environment = create_environment(&router, &application, "boundary-env").await;

    let unknown_application = "zzzzzzz";
    let rows: Vec<(&str, Answer, String)> = vec![
        (
            "a stop under an application the account does not hold",
            stop(&router, unknown_application, &environment, 1, None).await,
            format!(
                "Application with Id {unknown_application} could not be found for account \
                 {account}."
            ),
        ),
        (
            "a report of a deployment number nothing issued",
            get_deployment(&router, &application, &environment, 1).await,
            format!(
                "Deployment with Application Id {application}, Environment Id {environment}, and \
                 deploymentNumber 1 could not be found for account {account}."
            ),
        ),
        (
            "a report under an application the account does not hold",
            get_deployment(&router, unknown_application, &environment, 1).await,
            format!(
                "Application with Id {unknown_application} could not be found for account \
                 {account}."
            ),
        ),
    ];

    for (case, answer, expected) in rows {
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.string("Message"),
            Some(expected.clone()),
            "case {case:?}: expected {expected:?}, got {}",
            answer.json()
        );
    }
}
