// SPDX-License-Identifier: CC0-1.0

//! What a configuration session may be opened over, driven end to end over HTTP.
//!
//! The rule under test, which is about the data plane's precondition and not about one
//! payload, one identifier spelling or one profile: `StartConfigurationSession` is a
//! session over a DEPLOYED configuration, so a triple whose configuration profile has
//! never been deployed into the named environment is refused with the modelled
//! `ResourceNotFoundException` (404) reporting the `Deployment` as the missing resource,
//! and no token is minted. What makes a session possible is that the profile HAS been
//! deployed into that environment — whatever state that deployment is in — and the
//! refusal lifts the moment a deployment exists.
//!
//! Every case drives real requests through the router harness the polling suite sets up
//! ([`super::configuration_session_poll::Emulator`]), which mounts AppConfigData and the
//! AppConfig control plane over one shared engine: the applications, environments,
//! configuration profiles, hosted configuration versions and deployments are created
//! through AppConfig's own operations, exactly as a client would create them, so the
//! precondition is established from the emulator's own state rather than written in by
//! hand.

use bytes::Bytes;

use super::configuration_session_poll::{is_modelled_token, Emulator, Hierarchy};

/// The modelled `ResourceType` value for the resource that is missing when a profile has
/// never been deployed into an environment — one of the five the model's `ResourceType`
/// enum declares (`RESOURCE_TYPE_VALUES` in `src/ops/types/shared_types.rs`).
const RESOURCE_TYPE_DEPLOYMENT: &str = "Deployment";

/// The whole `Message` the refusal carries: fixed text with nothing interpolated into
/// it. Which parameters named the missing deployment travels in `ReferencedBy`.
const DEPLOYMENT_NOT_FOUND: &str = "Deployment not found";

/// The configuration profile every case starts a session over, and the sibling profile a
/// case may deploy instead.
const TARGET_PROFILE: &str = "TargetProfile";
/// See [`TARGET_PROFILE`].
const CONTROL_PROFILE: &str = "ControlProfile";

/// How a case spells the three identifiers of the triple it starts a session over: each
/// is documented as "the ID or the name" of its level, so the refusal must be the same
/// either way, and `ReferencedBy` must echo back whichever spelling the request used.
#[derive(Debug, Clone, Copy)]
enum Spelling {
    ByName,
    ById,
    /// The application by id, the environment by name, the profile by id.
    Mixed,
}

/// What exists around the triple under test, i.e. the reason it has no deployment.
#[derive(Debug, Clone, Copy)]
enum Setup {
    /// A sibling configuration profile in the same environment IS deployed; the profile
    /// under test is not. The deployed sibling is also the positive control.
    SiblingProfileDeployed,
    /// Nothing has been deployed into the environment at all.
    NothingDeployedInTheEnvironment,
    /// The very same configuration profile is deployed, but into ANOTHER environment of
    /// the application: a deployment is per environment, so this one is not one of the
    /// environment under test.
    DeployedIntoAnotherEnvironment,
}

/// One never-deployed triple. The rows differ along what the rule generalises over — why
/// the triple has no deployment, how the request spells its identifiers, and the form,
/// size and content type of the configuration eventually deployed to lift the refusal.
struct Case {
    name: &'static str,
    setup: Setup,
    spelling: Spelling,
    /// The `Type` the profile under test is created with; `None` declares none, which
    /// AppConfig stores as a freeform profile. Both serve their stored bytes unchanged.
    profile_type: Option<&'static str>,
    content: &'static [u8],
    content_type: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "a sibling profile is deployed, triple named by name, json document",
        setup: Setup::SiblingProfileDeployed,
        spelling: Spelling::ByName,
        profile_type: None,
        content: br#"{"restored":true}"#,
        content_type: "application/json",
    },
    Case {
        name: "a sibling profile is deployed, triple named by id, plain text",
        setup: Setup::SiblingProfileDeployed,
        spelling: Spelling::ById,
        profile_type: Some("AWS.Freeform"),
        content: b"on",
        content_type: "text/plain",
    },
    Case {
        name: "nothing deployed in the environment, ids and names mixed, yaml document",
        setup: Setup::NothingDeployedInTheEnvironment,
        spelling: Spelling::Mixed,
        profile_type: None,
        content: b"feature: on\nextra: 2\n",
        content_type: "application/x-yaml",
    },
    Case {
        name: "nothing deployed in the environment, triple named by name, non-utf8 blob",
        setup: Setup::NothingDeployedInTheEnvironment,
        spelling: Spelling::ByName,
        profile_type: None,
        content: &[0x00, 0x9f, 0x92, 0x96, 0xff],
        content_type: "application/octet-stream",
    },
    Case {
        name: "the profile is deployed into another environment, triple named by id, json",
        setup: Setup::DeployedIntoAnotherEnvironment,
        spelling: Spelling::ById,
        profile_type: None,
        content: br#"{"tier":"one"}"#,
        content_type: "application/json",
    },
];

/// A triple with no deployment of its configuration profile is refused the session, with
/// the modelled error, the `Deployment` resource type, the fixed message and every
/// request member that named the missing deployment — and nothing is minted. Deploying a
/// configuration into that environment lifts the refusal, and the token the session then
/// returns polls the deployed bytes.
#[tokio::test]
async fn a_session_over_a_triple_with_no_deployment_is_refused_until_one_exists() {
    for case in CASES {
        let name = case.name;
        let emulator = Emulator::new();
        let hierarchy = emulator
            .create_hierarchy("MyApp", "Prod", CONTROL_PROFILE)
            .await;
        let target_profile_id = emulator
            .create_typed_profile(&hierarchy.application_id, TARGET_PROFILE, case.profile_type)
            .await;

        match case.setup {
            Setup::SiblingProfileDeployed => {
                emulator
                    .deploy(
                        &hierarchy,
                        &hierarchy.profile_id,
                        b"the sibling's configuration",
                        "text/plain",
                        None,
                    )
                    .await;
                // Positive control: the sibling profile HAS been deployed into this
                // environment, so a session over it is minted.
                let control = emulator
                    .session_token(
                        &hierarchy.application_name,
                        &hierarchy.environment_name,
                        &hierarchy.profile_name,
                    )
                    .await;
                assert!(
                    is_modelled_token(&control),
                    "case {name}: a deployed profile must still be given a session, got {control:?}"
                );
            }
            Setup::NothingDeployedInTheEnvironment => {}
            Setup::DeployedIntoAnotherEnvironment => {
                let other_environment_id = emulator
                    .create_environment(&hierarchy.application_id, "Beta")
                    .await;
                let elsewhere = Hierarchy {
                    environment_id: other_environment_id,
                    environment_name: "Beta".to_string(),
                    ..hierarchy.clone()
                };
                emulator
                    .deploy(
                        &elsewhere,
                        &target_profile_id,
                        b"deployed somewhere else",
                        "text/plain",
                        None,
                    )
                    .await;
            }
        }

        let (application, environment, profile) =
            spell(case.spelling, &hierarchy, &target_profile_id);
        let sessions_before = emulator.state().lock().sessions.len();

        let refused = emulator
            .post_json(
                "/configurationsessions",
                &format!(
                    r#"{{"ApplicationIdentifier":"{application}",
                          "EnvironmentIdentifier":"{environment}",
                          "ConfigurationProfileIdentifier":"{profile}"}}"#
                ),
            )
            .await;
        assert_eq!(
            refused.status,
            404,
            "case {name}: expected 404, got {} body {}",
            refused.status,
            refused.text()
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {name}: expected ResourceNotFoundException, got {}",
            refused.text()
        );
        assert_eq!(
            refused.header("x-amzn-errortype"),
            Some("ResourceNotFoundException"),
            "case {name}: expected the error type header, got {:?}",
            refused.header("x-amzn-errortype")
        );
        let body = refused.json();
        assert_eq!(
            body["ResourceType"].as_str(),
            Some(RESOURCE_TYPE_DEPLOYMENT),
            "case {name}: expected the Deployment resource type, got {}",
            refused.text()
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(DEPLOYMENT_NOT_FOUND),
            "case {name}: expected the whole fixed message, got {}",
            refused.text()
        );
        // A deployment has no identifier of its own in the request: the triple is what
        // named it, so every one of the three members is reported as having referenced
        // it, each with the spelling the request used.
        for (member, identifier) in [
            ("ApplicationIdentifier", application.as_str()),
            ("EnvironmentIdentifier", environment.as_str()),
            ("ConfigurationProfileIdentifier", profile.as_str()),
        ] {
            assert_eq!(
                body["ReferencedBy"][member].as_str(),
                Some(identifier),
                "case {name}: expected ReferencedBy[{member}] to echo the request, got {}",
                refused.text()
            );
        }
        assert!(
            body["InitialConfigurationToken"].is_null(),
            "case {name}: a refused session must mint no token, got {}",
            refused.text()
        );
        assert_eq!(
            emulator.state().lock().sessions.len(),
            sessions_before,
            "case {name}: a refused session must persist nothing"
        );

        // The refusal is the precondition, not a verdict on the triple: once a
        // configuration is deployed into THIS environment for THIS profile, the same
        // call is served and its token polls the deployed bytes.
        emulator
            .deploy(
                &hierarchy,
                &target_profile_id,
                case.content,
                case.content_type,
                None,
            )
            .await;
        let served = emulator
            .post_json(
                "/configurationsessions",
                &format!(
                    r#"{{"ApplicationIdentifier":"{application}",
                          "EnvironmentIdentifier":"{environment}",
                          "ConfigurationProfileIdentifier":"{profile}"}}"#
                ),
            )
            .await;
        assert_eq!(
            served.status,
            201,
            "case {name}: a deployed triple must be given a session, got {} body {}",
            served.status,
            served.text()
        );
        let token = served.string("InitialConfigurationToken");
        assert!(
            is_modelled_token(&token),
            "case {name}: expected a well-formed token, got {token:?}"
        );
        let poll = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            poll.status,
            200,
            "case {name}: the poll expected 200, got {} body {}",
            poll.status,
            poll.text()
        );
        assert_eq!(
            poll.body.as_ref(),
            case.content,
            "case {name}: the poll must serve the deployed bytes, got {:?}",
            poll.body.as_ref()
        );
        assert_eq!(
            poll.header("content-type"),
            Some(case.content_type),
            "case {name}: the poll must serve the deployed content type, got {:?}",
            poll.header("content-type")
        );
    }
}

/// The state a deployment of the profile is in when the session is started, and the
/// `DeploymentState` AppConfig reports for it.
struct DeploymentStateCase {
    name: &'static str,
    /// Whether the deployment's rollout window and bake time are moved into the past,
    /// which is what finishes it.
    settled: bool,
    /// Whether a caller stops the deployment, which undoes the configuration it put in
    /// force while leaving the deployment itself in the environment.
    stopped: bool,
    /// The `DeploymentState` values this leaves the deployment in, any of which the
    /// control plane may legitimately report by the time it is read (a stop starts a
    /// rollback the clock then finishes). `DeploymentState` is listed for GetDeployment
    /// in the control plane's own `OPERATIONS.md`.
    states: &'static [&'static str],
}

const DEPLOYMENT_STATE_CASES: &[DeploymentStateCase] = &[
    DeploymentStateCase {
        name: "still monitoring its final bake time",
        settled: false,
        stopped: false,
        states: &["BAKING", "DEPLOYING"],
    },
    DeploymentStateCase {
        name: "finished",
        settled: true,
        stopped: false,
        states: &["COMPLETE"],
    },
    DeploymentStateCase {
        name: "undone by a caller's stop",
        settled: false,
        stopped: true,
        states: &["ROLLING_BACK", "ROLLED_BACK", "REVERTED"],
    },
];

/// What opens a session is that the configuration profile HAS been deployed into the
/// environment, whatever state that deployment is in: a deployment still rolling out, one
/// that finished, and one a caller undid are all deployments the environment holds, so
/// each is given a session — the currency of the configuration is a question for the poll,
/// not for the session.
#[tokio::test]
async fn a_session_is_minted_for_any_deployment_the_environment_holds() {
    for case in DEPLOYMENT_STATE_CASES {
        let name = case.name;
        let emulator = Emulator::new();
        let hierarchy = emulator
            .create_hierarchy("MyApp", "Prod", TARGET_PROFILE)
            .await;
        let number = if case.settled {
            emulator
                .deploy_numbered(
                    &hierarchy,
                    &hierarchy.profile_id,
                    b"deployed once",
                    "text/plain",
                    None,
                )
                .await
        } else {
            // The row needs the deployment still in flight, which is what it is until the
            // stages the clock owes it fall due.
            emulator
                .deploy_in_flight(
                    &hierarchy,
                    &hierarchy.profile_id,
                    b"deployed once",
                    "text/plain",
                    None,
                )
                .await
        };
        if case.stopped {
            emulator.stop_deployment(&hierarchy, number).await;
        }

        // The state is read back from the control plane, so the row asserts against
        // what AppConfig itself reports rather than against an assumption.
        let reported = deployment_state(&emulator, &hierarchy, number).await;
        assert!(
            case.states.contains(&reported.as_str()),
            "case {name}: expected one of {:?}, control plane reports {reported}",
            case.states
        );

        let token = emulator
            .session_token(
                &hierarchy.application_name,
                &hierarchy.environment_name,
                &hierarchy.profile_name,
            )
            .await;
        assert!(
            is_modelled_token(&token),
            "case {name}: a deployment in state {reported} must still open a session, \
             got {token:?}"
        );
    }
}

/// The `DeploymentState` the AppConfig control plane reports for one deployment.
async fn deployment_state(emulator: &Emulator, hierarchy: &Hierarchy, number: i64) -> String {
    let application_id = &hierarchy.application_id;
    let environment_id = &hierarchy.environment_id;
    let reply = emulator
        .send(
            "GET",
            &format!(
                "/applications/{application_id}/environments/{environment_id}\
                 /deployments/{number}"
            ),
            "application/json",
            &[],
            Bytes::new(),
        )
        .await;
    assert_eq!(
        reply.status,
        200,
        "GetDeployment({number}): expected 200, got {} body {}",
        reply.status,
        reply.text()
    );
    reply.string("State")
}

/// The three identifiers as a case spells them: ids, names, or a mixture.
fn spell(
    spelling: Spelling,
    hierarchy: &Hierarchy,
    target_profile_id: &str,
) -> (String, String, String) {
    match spelling {
        Spelling::ByName => (
            hierarchy.application_name.clone(),
            hierarchy.environment_name.clone(),
            TARGET_PROFILE.to_string(),
        ),
        Spelling::ById => (
            hierarchy.application_id.clone(),
            hierarchy.environment_id.clone(),
            target_profile_id.to_string(),
        ),
        Spelling::Mixed => (
            hierarchy.application_id.clone(),
            hierarchy.environment_name.clone(),
            target_profile_id.to_string(),
        ),
    }
}
