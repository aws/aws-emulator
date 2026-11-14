// SPDX-License-Identifier: CC0-1.0

//! The deprecated `GetConfiguration` read: which identifiers its refusal names.
//!
//! RULE: a refusal of this path names each level of the triple AS THE CALL RESOLVED IT — a
//! level the request addressed by name or by id and that RESOLVED is reported by the id the
//! lookup under it used, while a level that resolves to nothing has only the label the
//! request spelled and is echoed as written. Each path member is documented as "the <level>
//! name or the <level> ID" (`smithy_query(query="operation",
//! operation="GetConfiguration")`), so the label a caller sends and the id a lookup is made
//! with are two different strings, and the refusal reports the second wherever it has one.
//!
//! The rows span what the rule generalises over: which level is addressed by name and which
//! by id, and which level fails to resolve — including the triple that resolves whole with
//! nothing deployed into it, which is the same refusal. What this path answers a request that
//! omits its `@required` `client_id` is `omitted_required_bindings`, with the other member
//! this service's model binds that way.

use crate::appconfig_workflow::{
    account, create_application, create_environment, create_profile, router, send, Answer,
};
use smithy_server_runtime::MultiServiceRouter;

/// `GET` one configuration, naming the three levels as the row spells them.
async fn get_configuration(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    configuration: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!(
            "/applications/{application}/environments/{environment}/configurations/\
             {configuration}?client_id=test-client"
        ),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// The refusal this path states for a profile it has nothing to serve for.
fn expected_message(application: &str, configuration: &str) -> String {
    format!(
        "Configuration Profile with Application Id {application} and Configuration Profile Id \
         {configuration} could not be found for account {}.",
        account()
    )
}

/// RULE: the refusal names the application and the configuration profile as this call
/// resolved them — the resolved id where the identifier resolved, the label the request sent
/// where it did not.
#[tokio::test]
async fn the_refusal_names_each_level_as_the_call_resolved_it() {
    let router = router();
    let application_name = "resolved-identifiers";
    let application = create_application(&router, application_name).await;
    let environment_name = "production";
    let environment = create_environment(&router, &application, environment_name).await;
    let profile_name = "config";
    let profile = create_profile(&router, &application, profile_name).await;

    // (case, how the request spells the three levels, the application and the configuration
    // profile the refusal must name)
    let rows: [(&str, &str, &str, &str, String, String); 6] = [
        (
            "an application addressed by NAME, with a profile that does not resolve",
            application_name,
            environment_name,
            "nosuchprofile",
            application.clone(),
            "nosuchprofile".to_string(),
        ),
        (
            "an application addressed by its ID, with a profile that does not resolve",
            application.as_str(),
            environment_name,
            "nosuchprofile",
            application.clone(),
            "nosuchprofile".to_string(),
        ),
        (
            "an application that does not resolve",
            "nosuchapp",
            environment_name,
            profile_name,
            "nosuchapp".to_string(),
            profile_name.to_string(),
        ),
        (
            "every level addressed by name, with nothing deployed",
            application_name,
            environment_name,
            profile_name,
            application.clone(),
            profile.clone(),
        ),
        (
            "every level addressed by id, with nothing deployed",
            application.as_str(),
            environment.as_str(),
            profile.as_str(),
            application.clone(),
            profile.clone(),
        ),
        (
            "an environment addressed by a name nothing holds",
            application_name,
            "nosuchenv",
            profile_name,
            application.clone(),
            profile.clone(),
        ),
    ];

    for (case, named_application, named_environment, named_configuration, reported_application, reported_configuration) in
        &rows
    {
        let answer = get_configuration(
            &router,
            named_application,
            named_environment,
            named_configuration,
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let code = answer.error_code();
        assert!(
            code.contains("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {code:?}"
        );
        let expected = expected_message(reported_application, reported_configuration);
        assert_eq!(
            answer.string("Message").unwrap_or_default(),
            expected,
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            answer.string("Message")
        );
    }
}

