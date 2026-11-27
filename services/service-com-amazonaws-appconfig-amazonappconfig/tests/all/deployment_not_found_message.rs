// SPDX-License-Identifier: CC0-1.0

//! The `ResourceNotFoundException` a deployment lookup that resolves nothing is refused
//! with.
//!
//! The rule under test, in one sentence: AppConfig's not-found refusal labels each
//! identifier it echoes with the token that resource kind's own message uses — for a
//! deployment, the application and the environment by their title-cased `Application Id`
//! and `Environment Id`, and the deployment number by the lowerCamelCase WIRE spelling of
//! the member it came from, `deploymentNumber` — so a label is never derived by re-casing
//! every one of a template's labels the same way.
//!
//! Every row drives real HTTP requests through the router the emulator's front door uses,
//! so what is asserted is the `Message` an AppConfig client reads on the wire.
//!
//! The table spans what the rule generalises over rather than the one recorded lookup: the
//! number asked for (one the environment never issued, one a SIBLING environment did issue,
//! `0`, and the published bounds of the `Integer` member), which environment of the
//! application is addressed, and which operation does the lookup — `GetDeployment` and
//! `StopDeployment` both resolve a deployment number under an environment, so both are
//! asserted to word the miss with this sentence. The stop then relays the component it
//! settles a deployment through, which appends a clause after that sentence; the clause is
//! `crate::stop_deployment_relayed_not_found`'s subject, so what is asserted here is the
//! sentence the two operations share.
//!
//! The second suite pins the other half of the deployment REPORT's refusal: the resource
//! `GetDeployment` addresses is the deployment, so a path that resolves no deployment is
//! reported as the deployment that could not be found whichever level of it is missing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    account, create_application, create_environment, create_profile, json_call, router, send,
    Answer,
};

/// One predefined deployment strategy id: AppConfig's `AppConfig.*` strategies are
/// deployable as shipped, so a row needs no strategy of its own.
const ALL_AT_ONCE: &str = "AppConfig.AllAtOnce";

/// The upper bound of a `DeploymentNumber`, which the model targets at `Integer`
/// (`smithy_query(query="operation",operation="GetDeployment")`).
const MAX_DEPLOYMENT_NUMBER: i64 = i32::MAX as i64;

/// `GET /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}`.
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

/// `DELETE /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}`,
/// which is `StopDeployment`'s modelled route
/// (`smithy_query(query="operation",operation="StopDeployment")`).
async fn stop_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// Write one hosted configuration version and answer the version number it was given.
async fn given_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    content: &[u8],
) -> String {
    let created = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateHostedConfigurationVersion must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .header("Version-Number")
        .expect("CreateHostedConfigurationVersion answers a Version-Number")
}

/// The refusal `operation` must answer for the deployment sentence `sentence`.
///
/// `GetDeployment` REPORTS a deployment out of the control-plane tree, so its refusal is
/// that sentence and nothing else. `StopDeployment` settles the deployment through the
/// component that owns one in flight, so its refusal is that component's answer relayed:
/// the same sentence, then the clause naming the relayed call, every token of which is fixed
/// text except the per-call request id — whose own form is asserted by
/// `crate::stop_deployment_relayed_not_found`.
fn assert_deployment_refusal(
    case: &str,
    operation: &str,
    answer: &Answer,
    sentence: &str,
    relayed: bool,
) {
    let message = answer.string("Message").unwrap_or_default();
    if !relayed {
        assert_eq!(
            message, sentence,
            "case {case:?}: {operation} must answer the deployment template and nothing else, \
             got {message:?}"
        );
        return;
    }
    let head = format!(
        "{sentence} (Service: AWSAppConfigData; Status Code: 404; Error Code: \
         ResourceNotFoundException; Request ID: "
    );
    assert!(
        message.starts_with(&head),
        "case {case:?}: {operation} must answer the deployment template and the relayed \
         clause, expected it to start {head:?}, got {message:?}"
    );
    assert!(
        message.ends_with("; Proxy: null)"),
        "case {case:?}: {operation}'s relayed clause must end \"; Proxy: null)\", got {message:?}"
    );
}

/// A deployment number a lookup under `environment` must miss on, and the refusal both
/// readers of a deployment answer for it.
#[tokio::test]
async fn a_deployment_lookup_that_misses_names_its_number_by_the_wire_member_spelling() {
    let router = router();
    let account = account();
    let application = create_application(&router, "deployment-misses").await;
    // Two environments: one that has issued a deployment number and one that never has,
    // so the same number is a miss under one and a hit under the other.
    let deployed = create_environment(&router, &application, "deployed-env").await;
    let untouched = create_environment(&router, &application, "untouched-env").await;
    let profile = create_profile(&router, &application, "numbered").await;
    let version = given_version(&router, &application, &profile, br#"{"v":1}"#).await;

    let started = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments/{deployed}/deployments"),
        &json!({
            "DeploymentStrategyId": ALL_AT_ONCE,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": version,
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

    // Rows differ along the number asked for and the environment it is asked under; the
    // number 1 is a HIT under `deployed` and a miss under `untouched`, which is what makes
    // the lookup per environment rather than per application.
    let rows: Vec<(&str, &str, i64)> = vec![
        ("the next number the environment has not issued", &deployed, 2),
        (
            "a number a sibling environment issued",
            &untouched,
            1,
        ),
        ("the number below the sequence", &deployed, 0),
        ("a number far past the sequence", &deployed, 4096),
        (
            "the upper bound of the Integer member",
            &untouched,
            MAX_DEPLOYMENT_NUMBER,
        ),
    ];

    for (case, environment, number) in rows {
        let expected = format!(
            "Deployment with Application Id {application}, Environment Id {environment}, and \
             deploymentNumber {number} could not be found for account {account}."
        );
        // The last column is whether that operation's refusal is RELAYED from the component
        // the stop settles a deployment through, which appends its clause after the sentence.
        for (operation, answer, relayed) in [
            (
                "GetDeployment",
                get_deployment(&router, &application, environment, number).await,
                false,
            ),
            (
                "StopDeployment",
                stop_deployment(&router, &application, environment, number).await,
                true,
            ),
        ] {
            assert_eq!(
                answer.status,
                404,
                "case {case:?}: {operation} must answer 404 for a deployment nothing holds, \
                 got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {case:?}: {operation} must answer ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
            assert_deployment_refusal(case, operation, &answer, &expected, relayed);
        }
    }

    // The number that IS held is still served, so the rows above pin a miss rather than a
    // reader that misses on everything.
    let held = get_deployment(&router, &application, &deployed, 1).await;
    assert_eq!(
        held.status,
        200,
        "the deployment the environment issued must still be reported, got {} / {}",
        held.status,
        String::from_utf8_lossy(&held.body)
    );
}

/// A deployment REPORT whose path does not resolve BELOW the application is still reported
/// as the deployment that could not be found: the application the lookup is scoped to is
/// resolved first, as every operation addressed through an application id resolves it, and
/// under it the resource `GetDeployment` and `StopDeployment` address is the deployment — so
/// an environment that has since been deleted is answered with the deployment's own
/// template, not the environment's.
///
/// Rows differ along which level of the path is missing and how it came to be missing: an
/// environment that was deleted after it had issued the deployment, an environment that
/// never existed, and — the one level that keeps its own template — an application that
/// never existed.
///
/// `GetDeployment` reads a deployment THROUGH the environment that holds it, so it misses
/// on every row. `StopDeployment` resolves the deployment RECORD, whose lifetime is not the
/// environment's, so a record written before the environment was deleted is still there for
/// it to settle — what it answers for that record is asserted in
/// `crate::stop_deployment_without_its_environment` — and it misses only where no record of
/// that deployment was ever written.
#[tokio::test]
async fn a_deployment_report_names_the_deployment_whichever_level_of_the_path_is_missing() {
    let router = router();
    let account = account();
    let application = create_application(&router, "deployment-report-misses").await;
    let environment = create_environment(&router, &application, "doomed-env").await;
    let profile = create_profile(&router, &application, "reported").await;
    let version = given_version(&router, &application, &profile, br#"{"v":1}"#).await;

    let started = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": ALL_AT_ONCE,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": version,
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
    // The deployment is reported while the environment holds it, so what the rows below
    // assert is the effect of the path no longer resolving.
    assert_eq!(
        get_deployment(&router, &application, &environment, 1).await.status,
        200,
        "the deployment must be reported while its environment holds it"
    );
    let deleted = send(
        &router,
        "DELETE",
        &format!("/applications/{application}/environments/{environment}"),
        "application/json",
        Vec::new(),
        &[],
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "DeleteEnvironment must answer 204, got {} / {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    // The last column is whether a deployment RECORD was ever written under this path, so
    // `StopDeployment` has one to settle rather than a miss to report.
    let rows: Vec<(&str, &str, &str, bool)> = vec![
        (
            "the environment that issued it has been deleted",
            &application,
            &environment,
            true,
        ),
        (
            "an environment that never existed",
            &application,
            "nosuchenv",
            false,
        ),
    ];

    for (case, application_id, environment_id, record_written) in rows {
        let expected = format!(
            "Deployment with Application Id {application_id}, Environment Id {environment_id}, \
             and deploymentNumber 1 could not be found for account {account}."
        );
        // As above, the last column of each answer is whether that operation's refusal is
        // relayed from the component the stop settles a deployment through.
        let mut answers = vec![(
            "GetDeployment",
            get_deployment(&router, application_id, environment_id, 1).await,
            false,
        )];
        if !record_written {
            answers.push((
                "StopDeployment",
                stop_deployment(&router, application_id, environment_id, 1).await,
                true,
            ));
        }
        for (operation, answer, relayed) in answers {
            assert_eq!(
                answer.status,
                404,
                "case {case:?}: {operation} must answer 404, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_deployment_refusal(case, operation, &answer, &expected, relayed);
        }
    }

    // The APPLICATION level keeps its own template: it is the level every operation
    // addressed through an application id resolves first, as `GetEnvironment` under an
    // application the account does not hold reports it too.
    let case = "an application that never existed";
    let unknown_application = "zzzzzzz";
    for (operation, answer) in [
        (
            "GetDeployment",
            get_deployment(&router, unknown_application, &environment, 1).await,
        ),
        (
            "StopDeployment",
            stop_deployment(&router, unknown_application, &environment, 1).await,
        ),
    ] {
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: {operation} must answer 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.string("Message"),
            Some(format!(
                "Application with Id {unknown_application} could not be found for account \
                 {account}."
            )),
            "case {case:?}: {operation} must answer the application template, got {}",
            answer.json()
        );
    }
}
