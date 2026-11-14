// SPDX-License-Identifier: CC0-1.0

//! `StartDeployment`: how a deployment ADDRESSES the resources it deploys into.
//!
//! RULE: each resource a deployment names is addressed by its id or by its name, and the
//! deployment records and reports the ID the lookup resolved. The three members that name
//! them are typed to the length of those resources' NAMES — `ApplicationId` and
//! `EnvironmentId` target `Name` (1..64) and `ConfigurationProfileId` targets `LongName`
//! (1..128), none of them the `Id` shape `^[a-z0-9]{4,7}$` that this operation's own output
//! members carry (`smithy_query(query="operation", operation="StartDeployment")`) — so a
//! name is a value those members hold, and the id is what the started deployment reports
//! back for it.
//!
//! The rows span what the rule generalises over: WHICH of the three levels is spelled by
//! name (each on its own, and all three at once), and a name that resolves to nothing at
//! each level, which is the one refusal of that level.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    account, create_application, create_environment, create_profile, host_version, json_call,
    router, Answer,
};

/// A deployment strategy with no rollout window and no bake time, so each row's deployment
/// is finished when it is started and the next row is not refused for conflicting with it.
async fn create_instant_strategy(router: &MultiServiceRouter) -> String {
    json_call(
        router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": "instant-identifiers",
            "DeploymentDurationInMinutes": 0,
            "GrowthFactor": 100.0,
            "GrowthType": "LINEAR",
            "FinalBakeTimeInMinutes": 0,
            "ReplicateTo": "NONE",
        }),
    )
    .await
    .string("Id")
    .expect("CreateDeploymentStrategy answered no Id")
}

/// `StartDeployment`, naming the three levels as the row spells them.
async fn start_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
    strategy: &str,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await
}

/// RULE: a deployment resolves each resource it names by id or by name, and reports the
/// resolved ids — which a read of the deployment then reports too.
#[tokio::test]
async fn a_deployment_addresses_its_resources_by_id_or_by_name() {
    let router = router();
    let application_name = "deployment-identifiers";
    let application = create_application(&router, application_name).await;
    let environment_name = "production";
    let environment = create_environment(&router, &application, environment_name).await;
    let profile_name = "deployed-configuration";
    let profile = create_profile(&router, &application, profile_name).await;
    let strategy = create_instant_strategy(&router).await;
    host_version(
        &router,
        &application,
        &profile,
        "the version every row deploys",
        br#"{"enabled":true}"#,
        None,
    )
    .await;

    // (case, how the row spells the application, the environment and the profile)
    let rows: [(&str, &str, &str, &str); 5] = [
        (
            "every level by its id",
            application.as_str(),
            environment.as_str(),
            profile.as_str(),
        ),
        (
            "the configuration profile by its name",
            application.as_str(),
            environment.as_str(),
            profile_name,
        ),
        (
            "the environment by its name",
            application.as_str(),
            environment_name,
            profile.as_str(),
        ),
        (
            "the application by its name",
            application_name,
            environment.as_str(),
            profile.as_str(),
        ),
        (
            "every level by its name",
            application_name,
            environment_name,
            profile_name,
        ),
    ];

    for (case, named_application, named_environment, named_profile) in rows {
        let answer = start_deployment(
            &router,
            named_application,
            named_environment,
            named_profile,
            &strategy,
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        // The started deployment reports the ids the lookups resolved, whichever spelling
        // the request used.
        for (member, resolved) in [
            ("ApplicationId", &application),
            ("EnvironmentId", &environment),
            ("ConfigurationProfileId", &profile),
        ] {
            assert_eq!(
                answer.string(member).as_deref(),
                Some(resolved.as_str()),
                "case {case:?}: expected {member} {resolved:?}, got {:?}",
                answer.string(member)
            );
        }
        assert_eq!(
            answer.string("ConfigurationName").as_deref(),
            Some(profile_name),
            "case {case:?}: expected the profile's stored name, got {:?}",
            answer.string("ConfigurationName")
        );

        // The record really persisted under those ids: the deployment reads back through
        // the path the resolved ids address.
        let number = answer
            .json()
            .get("DeploymentNumber")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: no DeploymentNumber in {}", answer.json()));
        let read_back = json_call(
            &router,
            "GET",
            &format!(
                "/applications/{application}/environments/{environment}/deployments/{number}"
            ),
            &Value::Null,
        )
        .await;
        assert_eq!(
            read_back.status,
            200,
            "case {case:?}: GetDeployment must answer 200, got {} / {}",
            read_back.status,
            String::from_utf8_lossy(&read_back.body)
        );
        assert_eq!(
            read_back.string("ConfigurationProfileId").as_deref(),
            Some(profile.as_str()),
            "case {case:?}: expected the stored profile id on the read back, got {:?}",
            read_back.string("ConfigurationProfileId")
        );
    }
}

/// RULE: an identifier that resolves to NEITHER an id nor a name at its level is that
/// level's own not-found refusal, which names each level as the call RESOLVED it: the id
/// where the identifier resolved, the identifier as the request spelled it where it did not.
#[tokio::test]
async fn an_identifier_that_resolves_to_nothing_is_that_levels_refusal() {
    let router = router();
    let application_name = "unresolved-identifiers";
    let application = create_application(&router, application_name).await;
    let environment = create_environment(&router, &application, "production").await;
    let profile = create_profile(&router, &application, "config").await;
    let strategy = create_instant_strategy(&router).await;
    host_version(
        &router,
        &application,
        &profile,
        "the version the resolvable row deploys",
        br#"{"enabled":true}"#,
        None,
    )
    .await;
    let account = account();

    // (case, the three levels as the row spells them, the refusal it must answer)
    let rows: [(&str, &str, &str, &str, String); 5] = [
        (
            "an application neither id nor name resolves",
            "nosuchapp",
            environment.as_str(),
            profile.as_str(),
            format!("Application with Id nosuchapp could not be found for account {account}."),
        ),
        (
            "an environment neither id nor name resolves",
            application.as_str(),
            "nosuchenv",
            profile.as_str(),
            format!(
                "Environment with Application Id {application} and Environment Id nosuchenv \
                 could not be found for account {account}."
            ),
        ),
        (
            "a configuration profile neither id nor name resolves",
            application.as_str(),
            environment.as_str(),
            "nosuchprofile",
            format!(
                "Configuration Profile with Application Id {application} and Configuration \
                 Profile Id nosuchprofile could not be found for account {account}."
            ),
        ),
        // The application spelled by NAME with a child that resolves to nothing: the level
        // that resolved is named by the id it resolved to, not by the label the request sent.
        (
            "an application by name, with an environment that resolves to nothing",
            application_name,
            "nosuchenv",
            profile.as_str(),
            format!(
                "Environment with Application Id {application} and Environment Id nosuchenv \
                 could not be found for account {account}."
            ),
        ),
        (
            "an application by name, with a profile that resolves to nothing",
            application_name,
            environment.as_str(),
            "nosuchprofile",
            format!(
                "Configuration Profile with Application Id {application} and Configuration \
                 Profile Id nosuchprofile could not be found for account {account}."
            ),
        ),
    ];

    for (case, named_application, named_environment, named_profile, expected) in &rows {
        let answer = start_deployment(
            &router,
            named_application,
            named_environment,
            named_profile,
            &strategy,
        )
        .await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.string("Message").unwrap_or_default(),
            *expected,
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            answer.string("Message")
        );
    }
}
