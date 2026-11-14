// SPDX-License-Identifier: CC0-1.0

//! `StartDeployment`: the configuration version a deployment names has to be one the
//! configuration profile holds.
//!
//! RULE: an operation that acts on a configuration version resolves it against the versions
//! the named configuration profile's store actually holds BEFORE it writes anything, and
//! refuses one that resolves to none of them with the modelled
//! `ResourceNotFoundException` (HTTP 404) naming the configuration and the version the
//! request asked for — so no deployment record, and no listing of one, can report content
//! that does not exist. A hosted configuration version is named "either the version number
//! or version label" (`smithy_query(query="operation", operation="StartDeployment")` for
//! `ConfigurationVersion`, and `ResourceNotFoundException` among its errors), so both
//! spellings resolve; a profile whose configuration lives in another store — S3, Parameter
//! Store, Secrets Manager — holds no version here to resolve against, so this service makes
//! no such lookup for it.
//!
//! The rows span what the rule generalises over rather than one recorded call: the FORM of
//! the rejected value (a number past the versions created, a number before the first one, a
//! number that is not one, a label nothing carries, a label-shaped value, an over-long
//! value), the version a profile does hold (its number and its label, the first version and
//! a later one), and the store the profile's configuration lives in (hosted, and one this
//! service does not hold).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, create_profile_from, host_version,
    json_call, router, Answer, RETRIEVAL_ROLE,
};

/// A deployment strategy with no rollout window and no bake time, so each deployment it
/// paces is finished when it is started and the next one into the same environment is not
/// refused for conflicting with it — which is what a suite asserting a HISTORY of
/// deployments needs.
async fn create_instant_strategy(router: &MultiServiceRouter, case: &str) -> String {
    json_call(
        router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": format!("instant-{case}"),
            "DeploymentDurationInMinutes": 0,
            "GrowthFactor": 100.0,
            "GrowthType": "LINEAR",
            "FinalBakeTimeInMinutes": 0,
            "ReplicateTo": "NONE",
        }),
    )
    .await
    .string("Id")
    .unwrap_or_else(|| panic!("case {case:?}: CreateDeploymentStrategy answered no Id"))
}

/// `StartDeployment` of `configuration_version` of `profile` into `environment`.
async fn start_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
    strategy: &str,
    configuration_version: &str,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": configuration_version,
        }),
    )
    .await
}

/// Every deployment of `environment`, newest first — the listing a client audits what is
/// deployed from.
async fn list_deployments(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
) -> Vec<Value> {
    let answer = json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "ListDeployments must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The refusal AppConfig states for a configuration version a profile does not hold.
fn expected_message(configuration_name: &str, configuration_version: &str) -> String {
    format!("Could not find configuration {configuration_name} with version number {configuration_version}")
}

/// RULE: a `ConfigurationVersion` the named profile's hosted store never issued is refused
/// 404 `ResourceNotFoundException` naming the configuration and the version asked for, and
/// nothing is created: the environment issues no deployment number and its listing stays as
/// it was.
#[tokio::test]
async fn a_version_the_profile_does_not_hold_is_refused_and_deploys_nothing() {
    let router = router();
    let application = create_application(&router, "version-lookup").await;
    let environment = create_environment(&router, &application, "production").await;
    let strategy = create_instant_strategy(&router, "version-lookup").await;
    let profile_name = "config-under-test";
    let profile = create_profile(&router, &application, profile_name).await;
    host_version(
        &router,
        &application,
        &profile,
        "the version that exists",
        br#"{"enabled":true}"#,
        Some("release-one"),
    )
    .await;

    // Values that resolve to none of the versions the profile holds, differing along the
    // form of the value a caller can send: past the versions created, before the first one,
    // not a number at all, a label nothing carries, and a value at the member's declared
    // `@length` maximum (`Version`, len 1..1024).
    let rejected = [
        ("a version number past the ones created", "7".to_string()),
        ("the version number zero", "0".to_string()),
        ("a negative version number", "-1".to_string()),
        ("a version number that is not a number", "one".to_string()),
        (
            "a version label the profile does not carry",
            "release-two".to_string(),
        ),
        (
            "a value as long as the member allows",
            "v".repeat(1024),
        ),
    ];
    for (case, version) in &rejected {
        let answer =
            start_deployment(&router, &application, &environment, &profile, &strategy, version)
                .await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let code = answer.error_code();
        assert!(
            code.contains("ResourceNotFoundException"),
            "case {case:?}: expected the modelled ResourceNotFoundException, got {code:?}"
        );
        let expected = expected_message(profile_name, version);
        assert_eq!(
            answer.string("Message").unwrap_or_default(),
            expected,
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            answer.string("Message")
        );
        let listed = list_deployments(&router, &application, &environment).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused deployment must leave the listing empty, got {listed:?}"
        );
    }

    // The version the profile does hold deploys, by its number and by its label, and the
    // environment's deployment numbers start at one: nothing the refusals above consumed.
    for (case, version) in [
        ("the version by its number", "1"),
        ("the same version by its label", "release-one"),
    ] {
        let answer =
            start_deployment(&router, &application, &environment, &profile, &strategy, version)
                .await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.json().get("ConfigurationVersion").and_then(Value::as_str),
            Some(version),
            "case {case:?}: expected the deployed version reported back, got {}",
            answer.json()
        );
    }
    let numbers: Vec<i64> = list_deployments(&router, &application, &environment)
        .await
        .iter()
        .filter_map(|item| item.get("DeploymentNumber").and_then(Value::as_i64))
        .collect();
    assert_eq!(
        numbers,
        vec![2, 1],
        "expected the two deployments the profile's own versions started, got {numbers:?}"
    );
}

/// RULE: the listing reports, for each deployment, the configuration version that was
/// deployed and the `VersionLabel` the hosted version carries — the audit view of what is
/// deployed, read off the deployment record.
#[tokio::test]
async fn the_listing_reports_the_deployed_version_and_its_label() {
    let router = router();
    let application = create_application(&router, "listed-versions").await;
    let environment = create_environment(&router, &application, "production").await;
    let strategy = create_instant_strategy(&router, "listed-versions").await;
    let profile = create_profile(&router, &application, "config").await;

    // Two hosted versions, one labelled and one not, so the listing is asserted over a
    // deployment that has a label to report and one that has none.
    host_version(
        &router,
        &application,
        &profile,
        "version one",
        br#"{"enabled":true}"#,
        Some("first-cut"),
    )
    .await;
    host_version(
        &router,
        &application,
        &profile,
        "version two",
        br#"{"enabled":false}"#,
        None,
    )
    .await;

    // (case, the version the request names, the ConfigurationVersion the listing reports,
    // the VersionLabel it reports)
    let rows: [(&str, &str, &str, Option<&str>); 3] = [
        ("the first version by number", "1", "1", Some("first-cut")),
        ("the first version by label", "first-cut", "first-cut", Some("first-cut")),
        ("a version carrying no label", "2", "2", None),
    ];
    for (case, requested, reported, label) in rows {
        let answer = start_deployment(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            requested,
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let items = list_deployments(&router, &application, &environment).await;
        let newest = items
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the listing reported no deployment"));
        assert_eq!(
            newest.get("ConfigurationVersion").and_then(Value::as_str),
            Some(reported),
            "case {case:?}: expected the deployed version in the listing, got {newest}"
        );
        assert_eq!(
            newest.get("VersionLabel").and_then(Value::as_str),
            label,
            "case {case:?}: expected the hosted version's label in the listing, got {newest}"
        );
    }
}

/// RULE: a configuration profile whose configuration lives in a store this service does not
/// hold has no version here to resolve against, so the deployment is started with the
/// version as the request named it — the lookup above is made where AppConfig holds the
/// versions, not for every profile.
#[tokio::test]
async fn a_configuration_stored_elsewhere_has_no_version_to_resolve() {
    let router = router();
    let application = create_application(&router, "elsewhere").await;
    let environment = create_environment(&router, &application, "production").await;
    let strategy = create_instant_strategy(&router, "elsewhere").await;

    // Each location a profile can name whose bytes AppConfig does not store, and a version
    // of it that this service holds nothing for.
    for (case, location, version) in [
        ("a Parameter Store parameter", "ssm-parameter://probe-parameter", "3"),
        ("an S3 object", "s3://probe-bucket/probe-key", "11"),
        (
            "a Secrets Manager secret",
            "secretsmanager://probe-secret",
            "AWSCURRENT",
        ),
    ] {
        let profile = create_profile_from(
            &router,
            &application,
            case,
            &json!({
                "Name": format!("profile-for-{}", version),
                "LocationUri": location,
                "RetrievalRoleArn": RETRIEVAL_ROLE,
            }),
        )
        .await
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: CreateConfigurationProfile answered no Id"));
        let answer = start_deployment(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            version,
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.json().get("ConfigurationVersion").and_then(Value::as_str),
            Some(version),
            "case {case:?}: expected the version as the request named it, got {}",
            answer.json()
        );
    }
}
