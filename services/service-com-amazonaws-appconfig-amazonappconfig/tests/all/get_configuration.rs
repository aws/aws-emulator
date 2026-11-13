// SPDX-License-Identifier: CC0-1.0

//! `GetConfiguration`: the configuration AppConfig currently serves for one
//! (application, environment, configuration profile) triple.
//!
//! RULE: the deprecated configuration read answers the configuration the MOST RECENT
//! deployment in force FOR THE CALLING CLIENT put in service for the named configuration
//! profile in the named environment — a deployment's rollout reaches its targets
//! gradually, and `ClientId` is what places the caller among them, so a client a rollout
//! under way has not reached yet keeps being answered the configuration that preceded it
//! until the rollout grows past the client, and a caller's stop withdraws the new one
//! from every client — reporting that hosted version's number as `ConfigurationVersion`, its
//! MIME type as `ContentType`, and as `Content` whatever the profile's `Type` makes of
//! the version's stored bytes; it addresses each of the three levels by name or by id,
//! refuses every triple it has nothing to serve for — an unresolvable level as much as a
//! triple with no configuration in force — with the modelled `BadRequestException`, and
//! sends no content when the
//! caller's `ClientConfigurationVersion` is already the version in force
//! (`smithy_query(query="operation",operation="GetConfiguration")` for the members and
//! their bindings; the operation is documented as retrieving "the latest deployed
//! configuration", and `Configuration$Content` as containing "data if the system finds
//! new or updated configuration data … If there is no new or updated data and
//! ClientConfigurationVersion matches the version of the current configuration … the
//! Content value will be empty").
//!
//! Every case drives real HTTP requests through the router the emulator's front door
//! uses, so what is asserted is what an AppConfig client sees on the wire: the payload
//! bytes and the bound `Configuration-Version` / `Content-Type` headers.
//!
//! The tables span what the rule generalises over rather than one recorded call: the kind
//! of content a profile holds (a JSON document, text, bytes that are not text at all, and
//! a feature-flags profile whose served document is a projection of what is stored), every
//! spelling of the triple (each of the three levels addressed by id and by name), the
//! deployment history behind one profile (nothing deployed, one version, a newer version,
//! a withdrawn version, a version addressed by its label), each `ClientConfigurationVersion`
//! a client can send, and each level of the triple that can fail to resolve.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::{
    FEATURE_FLAGS_PROFILE_TYPE, FREEFORM_PROFILE_TYPE,
};

use crate::appconfig_workflow::{
    account, create_application, create_environment, create_profile_from, json_call, router, send,
    Answer, RETRIEVAL_ROLE,
};

/// How long a rollout under way is given to reach its targets before the suite fails.
///
/// A ceiling on the WAIT rather than a statement about the span a growth interval takes: a
/// rollout that has reached nobody by here is one nothing is advancing, which is the defect
/// this suite would be pinning.
const REACHED_BY: Duration = Duration::from_secs(120);

/// How often the read under observation is repeated while waiting for that condition.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The predefined deployment strategy that reaches every target at once: its rollout
/// window is empty, so a deployment it paces is already monitoring its bake time when
/// `StartDeployment` answers — the configuration it deployed is in force from that
/// instant.
const ALL_AT_ONCE: &str = "AppConfig.AllAtOnce";

/// A deployment strategy with no rollout window and no bake time, so each deployment it
/// paces is `COMPLETE` the instant it starts and the next one into the same environment
/// is not refused for conflicting with it — which is what a suite asserting a HISTORY of
/// deployments needs.
async fn create_instant_strategy(router: &MultiServiceRouter, case: &str) -> String {
    json_call(
        router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": "instant",
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

/// Where `GetConfiguration` reads one configuration, from the operation's modelled URI
/// (`smithy_query(query="operation",operation="GetConfiguration")`:
/// `/applications/{Application}/environments/{Environment}/configurations/{Configuration}`,
/// with `ClientId` bound to the `client_id` query parameter and
/// `ClientConfigurationVersion` to `client_configuration_version`).
fn configuration_uri(
    application: &str,
    environment: &str,
    configuration: &str,
    client_id: &str,
    client_configuration_version: Option<&str>,
) -> String {
    let mut uri = format!(
        "/applications/{application}/environments/{environment}/configurations/{configuration}\
         ?client_id={client_id}"
    );
    if let Some(version) = client_configuration_version {
        uri.push_str(&format!("&client_configuration_version={version}"));
    }
    uri
}

/// `GET` one configuration.
async fn get_configuration(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    configuration: &str,
    client_configuration_version: Option<&str>,
) -> Answer {
    send(
        router,
        "GET",
        &configuration_uri(
            application,
            environment,
            configuration,
            "test-client",
            client_configuration_version,
        ),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// Create a configuration profile of `profile_type` in the hosted store, answering its
/// minted id.
async fn create_hosted_profile(
    router: &MultiServiceRouter,
    application: &str,
    name: &str,
    profile_type: &str,
) -> String {
    create_profile_from(
        router,
        application,
        name,
        &json!({ "Name": name, "LocationUri": "hosted", "Type": profile_type }),
    )
    .await
    .string("Id")
    .unwrap_or_else(|| panic!("case {name:?}: CreateConfigurationProfile answered no Id"))
}

/// Write one hosted configuration version, answering the version number it was stored
/// under.
async fn host_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    case: &str,
    content_type: &str,
    content: &[u8],
    version_label: Option<&str>,
) -> i32 {
    let headers: Vec<(&str, String)> = version_label
        .map(|label| vec![("VersionLabel", label.to_string())])
        .unwrap_or_default();
    let answer = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        content_type,
        content.to_vec(),
        &headers,
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: CreateHostedConfigurationVersion must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .header("Version-Number")
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("case {case:?}: no Version-Number on the create response"))
}

/// `StartDeployment` of `configuration_version` of `profile` into `environment`.
async fn deploy(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
    strategy: &str,
    configuration_version: &str,
    case: &str,
) -> i64 {
    let answer = json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": configuration_version,
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: StartDeployment must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("DeploymentNumber")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: StartDeployment answered no DeploymentNumber"))
}

/// `StopDeployment`, which withdraws the configuration a deployment put in service.
async fn stop_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
    allow_revert: bool,
    case: &str,
) {
    let answer = send(
        router,
        "DELETE",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        "application/json",
        Vec::new(),
        &[("Allow-Revert", allow_revert.to_string())],
    )
    .await;
    assert_eq!(
        answer.status,
        202,
        "case {case:?}: StopDeployment must answer its modelled 202, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// One served configuration: its payload bytes and both bound headers.
fn assert_served(
    case: &str,
    answer: &Answer,
    expected_content: &[u8],
    expected_version: &str,
    expected_content_type: &str,
) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: GetConfiguration must answer the modelled 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.body.as_ref(),
        expected_content,
        "case {case:?}: expected the deployed content {:?}, got {:?}",
        String::from_utf8_lossy(expected_content),
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.header("Configuration-Version").as_deref(),
        Some(expected_version),
        "case {case:?}: expected ConfigurationVersion {expected_version:?}, got {:?}",
        answer.header("Configuration-Version")
    );
    assert_eq!(
        answer.header("Content-Type").as_deref(),
        Some(expected_content_type),
        "case {case:?}: expected ContentType {expected_content_type:?}, got {:?}",
        answer.header("Content-Type")
    );
}

/// One modelled refusal: the status and error code on the wire where an SDK classifies
/// it from, and the ONE message this deprecated read states for every triple it has nothing
/// to serve for — built from the application and configuration the REQUEST named, and the
/// account the lookup was scoped to.
fn assert_nothing_to_serve(case: &str, answer: &Answer, application: &str, configuration: &str) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected the modelled BadRequestException's HTTP 400, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains("BadRequestException"),
        "case {case:?}: expected BadRequestException, got {:?}",
        answer.error_code()
    );
    let expected = format!(
        "Configuration Profile with Application Id {application} and Configuration Profile Id \
         {configuration} could not be found for account {}.",
        account()
    );
    assert_eq!(
        answer.string("Message").unwrap_or_default(),
        expected,
        "case {case:?}: expected the refusal {expected:?}, got {:?}",
        answer.string("Message")
    );
}

/// One kind of content a configuration profile can hold.
struct Content {
    case: &'static str,
    /// The profile `Type` the content belongs to.
    profile_type: &'static str,
    /// The MIME type the hosted version is written with, which the read must report.
    content_type: &'static str,
    /// The bytes written into the hosted configuration version.
    stored: Vec<u8>,
    /// The bytes a consumer of the profile is served for them: the stored bytes for a
    /// freeform profile, and for a feature-flags profile the resolved flag document
    /// rather than the authoring document that is stored.
    served: Vec<u8>,
}

/// The content classes the rule is asserted over.
fn contents() -> Vec<Content> {
    let authoring = json!({
        "flags": { "newCheckout": { "name": "newCheckout" } },
        "values": { "newCheckout": { "enabled": true, "rolloutPercent": 25 } },
        "version": "1",
    });
    vec![
        Content {
            case: "a freeform JSON document",
            profile_type: FREEFORM_PROFILE_TYPE,
            content_type: "application/json",
            stored: br#"{"maxThreads":4,"featureOn":true}"#.to_vec(),
            served: br#"{"maxThreads":4,"featureOn":true}"#.to_vec(),
        },
        Content {
            case: "freeform text",
            profile_type: FREEFORM_PROFILE_TYPE,
            content_type: "text/plain",
            stored: b"max_threads = 4\nfeature_on = true\n".to_vec(),
            served: b"max_threads = 4\nfeature_on = true\n".to_vec(),
        },
        Content {
            case: "freeform bytes that are not text",
            profile_type: FREEFORM_PROFILE_TYPE,
            content_type: "application/octet-stream",
            stored: vec![0x00, 0x01, 0xfe, 0xff, 0x7f, 0x80],
            served: vec![0x00, 0x01, 0xfe, 0xff, 0x7f, 0x80],
        },
        Content {
            case: "a feature-flags authoring document",
            profile_type: FEATURE_FLAGS_PROFILE_TYPE,
            content_type: "application/json",
            stored: serde_json::to_vec(&authoring).expect("the document must serialize"),
            // The flag VALUES, which is what retrieving a feature-flag configuration
            // answers: the authoring half (`flags`, the schema `version` and the helper's
            // own stamps) belongs to authoring and is not served.
            served: serde_json::to_vec(&json!({
                "newCheckout": { "enabled": true, "rolloutPercent": 25 },
            }))
            .expect("the document must serialize"),
        },
    ]
}

/// Whether a level is addressed by its name or by its minted id, for the eight spellings
/// of a triple whose every level is documented as "the <level> name or the <level> ID".
fn addressed<'a>(by_name: bool, name: &'a str, id: &'a str) -> &'a str {
    if by_name {
        name
    } else {
        id
    }
}

/// RULE: whatever a profile holds, the read answers the version the deployment in force
/// put in service — the profile's projection of its stored bytes, that version's number
/// and its MIME type — and it answers the same whichever of the eight ways the triple is
/// addressed.
#[tokio::test]
async fn the_configuration_in_force_is_served_for_every_spelling_of_the_triple() {
    let router = router();

    for content in contents() {
        let case = content.case;
        let application_name = format!("app-{}", case.replace(' ', "-"));
        let environment_name = "production";
        let profile_name = "served";

        let application = create_application(&router, &application_name).await;
        let environment = create_environment(&router, &application, environment_name).await;
        let profile =
            create_hosted_profile(&router, &application, profile_name, content.profile_type).await;
        let version = host_version(
            &router,
            &application,
            &profile,
            case,
            content.content_type,
            &content.stored,
            None,
        )
        .await;
        deploy(
            &router,
            &application,
            &environment,
            &profile,
            ALL_AT_ONCE,
            &version.to_string(),
            case,
        )
        .await;

        for application_by_name in [false, true] {
            for environment_by_name in [false, true] {
                for configuration_by_name in [false, true] {
                    let spelling = format!(
                        "{case} / application by {}, environment by {}, configuration by {}",
                        if application_by_name { "name" } else { "id" },
                        if environment_by_name { "name" } else { "id" },
                        if configuration_by_name { "name" } else { "id" },
                    );
                    let answer = get_configuration(
                        &router,
                        addressed(application_by_name, &application_name, &application),
                        addressed(environment_by_name, environment_name, &environment),
                        addressed(configuration_by_name, profile_name, &profile),
                        None,
                    )
                    .await;
                    assert_served(
                        &spelling,
                        &answer,
                        &content.served,
                        &version.to_string(),
                        content.content_type,
                    );
                }
            }
        }

        // The version itself is untouched by being served: the hosted configuration
        // version still reports the bytes that were written, which for a feature-flags
        // profile are the authoring document rather than the document served above.
        let stored = send(
            &router,
            "GET",
            &format!(
                "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions/{version}"
            ),
            "application/json",
            Vec::new(),
            &[],
        )
        .await;
        assert_eq!(
            stored.status,
            200,
            "case {case:?}: GetHostedConfigurationVersion must answer 200, got {} / {}",
            stored.status,
            String::from_utf8_lossy(&stored.body)
        );
        let reported = String::from_utf8_lossy(&stored.body).to_string();
        if content.profile_type == FEATURE_FLAGS_PROFILE_TYPE {
            // A feature-flags profile stores the AUTHORING document — the flag
            // definitions and the authoring schema version, which the helper also stamps
            // — and serves the resolved flag values, so the two views differ and the
            // authoring halves appear only in the stored one.
            for authoring in ["flags", "version", "newCheckout"] {
                assert!(
                    reported.contains(authoring),
                    "case {case:?}: the stored version is the authoring document, so it must \
                     carry {authoring:?}, got {reported:?}"
                );
            }
            assert_ne!(
                stored.body.as_ref(),
                content.served.as_slice(),
                "case {case:?}: the authoring document a version stores is not the document a \
                 consumer is served"
            );
        } else {
            assert_eq!(
                stored.body.as_ref(),
                content.stored.as_slice(),
                "case {case:?}: the stored version must read back byte for byte, got {reported:?}"
            );
        }
    }
}

/// RULE: what is served follows the deployment history — the newest deployment still in
/// force decides, a withdrawn deployment stops deciding, and a version deployed by its
/// LABEL is served under its own version NUMBER.
#[tokio::test]
async fn the_newest_deployment_still_in_force_decides_what_is_served() {
    let router = router();
    let application = create_application(&router, "rollouts").await;
    let environment = create_environment(&router, &application, "production").await;
    let profile = create_hosted_profile(&router, &application, "rolled", FREEFORM_PROFILE_TYPE).await;
    let strategy = create_instant_strategy(&router, "history").await;

    // Three versions of one configuration, plus a fourth carrying a version label: the
    // model documents `StartDeployment`'s `ConfigurationVersion` as "either the version
    // number or version label" of a hosted version.
    for (number, body) in [(1, "one"), (2, "two"), (3, "three")] {
        let written = host_version(
            &router,
            &application,
            &profile,
            "history",
            "text/plain",
            body.as_bytes(),
            None,
        )
        .await;
        assert_eq!(
            written, number,
            "case \"history\": hosted versions are numbered from 1, got {written}"
        );
    }
    let labelled = host_version(
        &router,
        &application,
        &profile,
        "history",
        "text/plain",
        b"four",
        Some("release-four"),
    )
    .await;

    // Nothing deployed yet: the triple resolves and no configuration is in force.
    assert_nothing_to_serve(
        "nothing deployed",
        &get_configuration(&router, &application, &environment, &profile, None).await,
        &application,
        &profile,
    );

    // The first deployment puts version 1 in service.
    deploy(
        &router,
        &application,
        &environment,
        &profile,
        &strategy,
        "1",
        "the first deployment",
    )
    .await;
    assert_served(
        "the first deployment",
        &get_configuration(&router, &application, &environment, &profile, None).await,
        b"one",
        "1",
        "text/plain",
    );

    // A second deployment supersedes it.
    let second = deploy(
        &router,
        &application,
        &environment,
        &profile,
        &strategy,
        "2",
        "a newer deployment",
    )
    .await;
    assert_served(
        "a newer deployment",
        &get_configuration(&router, &application, &environment, &profile, None).await,
        b"two",
        "2",
        "text/plain",
    );

    // Reverting that deployment withdraws its configuration, so the one that preceded it
    // is in force again.
    stop_deployment(
        &router,
        &application,
        &environment,
        second,
        true,
        "a reverted deployment",
    )
    .await;
    assert_served(
        "a reverted deployment",
        &get_configuration(&router, &application, &environment, &profile, None).await,
        b"one",
        "1",
        "text/plain",
    );

    // A version deployed by its LABEL is the same stored version, so it is served under
    // its version number.
    deploy(
        &router,
        &application,
        &environment,
        &profile,
        &strategy,
        "release-four",
        "a deployment addressed by version label",
    )
    .await;
    assert_served(
        "a deployment addressed by version label",
        &get_configuration(&router, &application, &environment, &profile, None).await,
        b"four",
        &labelled.to_string(),
        "text/plain",
    );
}

/// RULE: `ClientConfigurationVersion` decides only whether the content is sent — the
/// version in force is reported either way, and only a client that already holds that
/// exact version is sent no content.
#[tokio::test]
async fn client_configuration_version_decides_whether_the_content_is_sent() {
    let router = router();
    let application = create_application(&router, "polling").await;
    let environment = create_environment(&router, &application, "production").await;
    let profile = create_hosted_profile(&router, &application, "polled", FREEFORM_PROFILE_TYPE).await;
    for body in ["one", "two"] {
        host_version(
            &router,
            &application,
            &profile,
            "polling",
            "text/plain",
            body.as_bytes(),
            None,
        )
        .await;
    }
    deploy(
        &router,
        &application,
        &environment,
        &profile,
        ALL_AT_ONCE,
        "2",
        "polling",
    )
    .await;

    // Every way a client can report the version it holds: none at all, the version in
    // force, an older version of the same profile, a version number that was never
    // issued, and a value that is not a version number.
    for (case, sent, expected) in [
        ("no client version", None, Some("two")),
        ("the version in force", Some("2"), None),
        ("an older version", Some("1"), Some("two")),
        ("a version never issued", Some("99"), Some("two")),
        ("not a version number", Some("release-one"), Some("two")),
    ] {
        let answer = get_configuration(&router, &application, &environment, &profile, sent).await;
        assert_served(
            case,
            &answer,
            expected.map(str::as_bytes).unwrap_or_default(),
            "2",
            "text/plain",
        );
    }
}

/// RULE: every triple this deprecated read has nothing to serve for — an unresolvable
/// level, a triple with no configuration in force, and a deployed version AppConfig's
/// hosted store does not hold — is ONE refusal: the modelled `BadRequestException`
/// (HTTP 400) naming the configuration profile it could not serve under the application, as
/// the request spelled the two of them, and the account the lookup was scoped to.
#[tokio::test]
async fn every_unservable_triple_is_the_one_modelled_bad_request() {
    let router = router();
    let application = create_application(&router, "refusals").await;
    let environment = create_environment(&router, &application, "production").await;
    let profile = create_hosted_profile(&router, &application, "deployed", FREEFORM_PROFILE_TYPE).await;
    host_version(
        &router,
        &application,
        &profile,
        "refusals",
        "text/plain",
        b"one",
        None,
    )
    .await;
    deploy(
        &router,
        &application,
        &environment,
        &profile,
        ALL_AT_ONCE,
        "1",
        "refusals",
    )
    .await;

    // Each level of the triple, addressed as something the store does not hold: whichever
    // level failed, the refusal reports the configuration profile it could not serve under
    // the application, so a caller cannot tell from it which level was the unresolvable
    // one.
    for (case, named_application, named_environment, named_configuration) in [
        (
            "an application the account does not hold",
            "nosuchapp",
            environment.as_str(),
            profile.as_str(),
        ),
        (
            "an environment the application does not hold",
            application.as_str(),
            "nosuchenv",
            profile.as_str(),
        ),
        (
            "a configuration profile the application does not hold",
            application.as_str(),
            environment.as_str(),
            "nosuchprofile",
        ),
        (
            "neither the environment nor the configuration profile",
            application.as_str(),
            "nosuchenv",
            "nosuchprofile",
        ),
    ] {
        let answer = get_configuration(
            &router,
            named_application,
            named_environment,
            named_configuration,
            None,
        )
        .await;
        assert_nothing_to_serve(case, &answer, named_application, named_configuration);
    }

    // A profile that resolves under an environment nothing was deployed into: the same
    // refusal, so an unknown environment and a real one with nothing deployed are one
    // answer.
    let other_environment = create_environment(&router, &application, "staging").await;
    assert_nothing_to_serve(
        "an environment with nothing deployed into it",
        &get_configuration(&router, &application, &other_environment, &profile, None).await,
        &application,
        &profile,
    );

    // A profile stored outside AppConfig's hosted configuration store: the control plane
    // holds the deployment, and the bytes it deployed are not AppConfig's to serve.
    let elsewhere = create_profile_from(
        &router,
        &application,
        "elsewhere",
        &json!({
            "Name": "elsewhere",
            "LocationUri": "ssm-parameter://probe-parameter",
            "RetrievalRoleArn": RETRIEVAL_ROLE,
        }),
    )
    .await
    .string("Id")
    .unwrap_or_else(|| panic!("case \"elsewhere\": CreateConfigurationProfile answered no Id"));
    deploy(
        &router,
        &application,
        &other_environment,
        &elsewhere,
        ALL_AT_ONCE,
        "1",
        "a configuration stored outside the hosted store",
    )
    .await;
    assert_nothing_to_serve(
        "a configuration stored outside the hosted store",
        &get_configuration(&router, &application, &other_environment, &elsewhere, None).await,
        &application,
        &elsewhere,
    );
}

/// RULE: a configuration is in force for the targets its rollout has REACHED — so a
/// rollout that has reached none of them serves none, one that has reached all of them
/// serves every client while it is still in flight — and a caller's stop withdraws it.
#[tokio::test]
async fn a_rolling_out_configuration_is_in_force_and_a_stop_withdraws_it() {
    let router = router();
    let application = create_application(&router, "pacing").await;
    let environment = create_environment(&router, &application, "production").await;
    let profile = create_hosted_profile(&router, &application, "paced", FREEFORM_PROFILE_TYPE).await;
    host_version(
        &router,
        &application,
        &profile,
        "pacing",
        "text/plain",
        b"gradual",
        None,
    )
    .await;

    // A strategy that declares a rollout window and a bake time, so the deployment is
    // still in flight throughout: its one growth step falls due at the end of that
    // window (`state::stage_duration`), which is what makes both halves of the rule
    // observable — nothing reached at the start instant, every target reached once the
    // window has passed, and the deployment baking rather than complete either way.
    let strategy = json_call(
        &router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": "slow-rollout",
            "DeploymentDurationInMinutes": 1440,
            "GrowthFactor": 100.0,
            "GrowthType": "LINEAR",
            "FinalBakeTimeInMinutes": 1440,
            "ReplicateTo": "NONE",
        }),
    )
    .await
    .string("Id")
    .unwrap_or_else(|| panic!("case \"pacing\": CreateDeploymentStrategy answered no Id"));

    let number = deploy(
        &router,
        &application,
        &environment,
        &profile,
        &strategy,
        "1",
        "a rollout under way",
    )
    .await;
    // The rollout has reached nobody yet, and nothing preceded it here, so there is no
    // configuration in force for this caller.
    assert_nothing_to_serve(
        "a rollout that has reached nobody",
        &get_configuration(&router, &application, &environment, &profile, None).await,
        &application,
        &profile,
    );

    // Once the rollout window has passed every target is reached, while the deployment
    // is still in flight (it is baking): the configuration is in force for every client.
    // The CONDITION is what is waited for — the read serving the deployed bytes — rather
    // than a fixed span, so the suite does not depend on how fast the host runs.
    let deadline = Instant::now() + REACHED_BY;
    let served = loop {
        let answer = get_configuration(&router, &application, &environment, &profile, None).await;
        if answer.status == 200 {
            break answer;
        }
        assert!(
            Instant::now() < deadline,
            "case \"a rollout that reached every target\": the rollout reached no target \
             within {}s; the read still answers {} / {}",
            REACHED_BY.as_secs(),
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_served(
        "a rollout that reached every target",
        &served,
        b"gradual",
        "1",
        "text/plain",
    );

    // Stopping it starts the rollback, which returns to the configuration that preceded
    // it — and nothing preceded it here, so no configuration is in force.
    stop_deployment(
        &router,
        &application,
        &environment,
        number,
        false,
        "a stopped rollout",
    )
    .await;
    assert_nothing_to_serve(
        "a stopped rollout",
        &get_configuration(&router, &application, &environment, &profile, None).await,
        &application,
        &profile,
    );
}
