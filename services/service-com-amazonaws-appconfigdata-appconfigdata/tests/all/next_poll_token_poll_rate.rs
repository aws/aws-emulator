// SPDX-License-Identifier: CC0-1.0

//! The poll-rate obligation a next-poll token carries, driven end to end over HTTP.
//!
//! The rule under test, which is about the session's negotiated interval and not about
//! one payload, one identifier or one interval value: a configuration token may not be
//! presented before the poll interval the session negotiated has elapsed since the
//! token was minted. Such a presentation is refused with the modelled
//! `BadRequestException` (400), and the refusal has NO effect — it serves no
//! configuration, rotates no token and records nothing, so the very same token is
//! accepted once the interval has passed. A session that negotiated no floor of its own
//! has nothing to enforce, and a brand-new session is never made to wait: the
//! obligation is per token, not per configuration.
//!
//! Every case drives real requests through the router harness the polling suite sets up
//! ([`super::configuration_session_poll::Emulator`]), which mounts AppConfigData and the
//! AppConfig control plane over one shared engine. The configuration is written and
//! deployed through AppConfig's own operations with a caller-created deployment strategy
//! that reaches every target at once, and the control plane is asked for the
//! deployment's state before the data plane is polled, so a poll is only ever made
//! against a deployment AppConfig itself reports finished.
//!
//! The one thing no operation exposes is the passage of time, so the instant a token was
//! minted is moved back in the session store ([`set_poll_instant`]) rather than slept
//! through: the refusal is the clock's, and a test must not spend the interval it is
//! asserting.

use std::time::{Duration, SystemTime};

use bytes::Bytes;
use service_com_amazonaws_appconfigdata_appconfigdata::state::DEFAULT_POLL_INTERVAL_IN_SECONDS;

use super::configuration_session_poll::{Emulator, Hierarchy, Reply};

/// A caller-created deployment strategy that reaches every target at once and bakes for
/// no time: `DeploymentDurationInMinutes` and `FinalBakeTimeInMinutes` are modelled
/// `range 0..1440` and `GrowthFactor` `range 1..100`, so this is the fastest strategy the
/// control plane models, and the deployment it paces is finished the moment it starts.
const INSTANT_STRATEGY: &str = r#"{"Name":"EveryTargetAtOnce",
                                   "DeploymentDurationInMinutes":0,
                                   "GrowthFactor":100,
                                   "FinalBakeTimeInMinutes":0,
                                   "ReplicateTo":"NONE"}"#;

/// The `DeploymentState` value AppConfig reports for a deployment whose rollout and bake
/// time are both over (`DeploymentState` in
/// `services/service-com-amazonaws-appconfig-amazonappconfig/OPERATIONS.md`, GetDeployment).
const DEPLOYMENT_COMPLETE: &str = "COMPLETE";

/// How much of an interval is left when a poll is made "just before" its interval is up.
///
/// A request through the harness takes a noticeable fraction of a second, and the clock
/// keeps running while it does, so the slice left has to be wider than one dispatch for
/// the poll to still be inside the interval when the handler reads the clock. It is a
/// property of the test harness, not of the rule: the rule is about the boundary, which
/// the accepted poll below sits exactly on.
const MARGIN: Duration = Duration::from_secs(3);

/// One negotiated poll rate. The rows differ along what the rule generalises over — the
/// interval the session asks for (each published bound of the modelled
/// `range 15..86400`, a value inside it, and no request at all) and the form, size and
/// content type of the configuration being polled — because none of that changes the
/// obligation.
struct PollRateCase {
    name: &'static str,
    /// `RequiredMinimumPollIntervalInSeconds` the session asks for; `None` for a session
    /// that asks for no floor, which therefore has none to enforce.
    requested: Option<i32>,
    /// The interval the poll must report, which is also the one it enforces.
    reported: i32,
    content: &'static [u8],
    content_type: &'static str,
}

const POLL_RATE_CASES: &[PollRateCase] = &[
    PollRateCase {
        name: "the modelled minimum interval, plain text",
        requested: Some(15),
        reported: 15,
        content: b"on",
        content_type: "text/plain",
    },
    PollRateCase {
        name: "a one-minute interval, json document",
        requested: Some(60),
        reported: 60,
        content: br#"{"tier":"one"}"#,
        content_type: "application/json",
    },
    PollRateCase {
        name: "a ten-minute interval, yaml document",
        requested: Some(600),
        reported: 600,
        content: b"tier: one\nextra: 2\n",
        content_type: "application/x-yaml",
    },
    PollRateCase {
        name: "the modelled maximum interval, non-utf8 blob",
        requested: Some(86_400),
        reported: 86_400,
        content: &[0x00, 0x9f, 0x92, 0x96, 0xff],
        content_type: "application/octet-stream",
    },
    PollRateCase {
        name: "no interval requested, json document",
        requested: None,
        reported: DEFAULT_POLL_INTERVAL_IN_SECONDS,
        content: br#"{"tier":"two"}"#,
        content_type: "application/json",
    },
];

/// An early presentation is refused, and a session that negotiated a floor may not poll
/// faster than the interval it was told to wait: the refusal repeats for as long as the
/// interval has not elapsed, leaves the token usable, and gives way to an accepted poll
/// the moment it has.
#[tokio::test]
async fn a_next_poll_token_may_not_be_presented_before_its_interval_has_elapsed() {
    for case in POLL_RATE_CASES {
        let name = case.name;
        let emulator = Emulator::new();
        let hierarchy = deploy_through_a_custom_strategy(&emulator, case).await;

        // The first poll of a session is always served: no token of its own has been
        // minted yet, so there is no interval to have elapsed.
        let initial = start_session(&emulator, &hierarchy, case.requested).await;
        let first = emulator.get_latest_configuration(&initial).await;
        assert_eq!(
            first.status,
            200,
            "case {name}: the first poll expected 200, got {} body {}",
            first.status,
            first.text()
        );
        assert_eq!(
            first.body.as_ref(),
            case.content,
            "case {name}: the first poll must serve the deployed bytes, got {:?}",
            first.body
        );
        let reported = case.reported.to_string();
        assert_eq!(
            first.header("next-poll-interval-in-seconds"),
            Some(reported.as_str()),
            "case {name}: expected a {reported}s obligation, got {:?}",
            first.header("next-poll-interval-in-seconds")
        );
        let next = first.next_token();
        assert_ne!(
            next, initial,
            "case {name}: the poll must rotate the token, got {next} back"
        );

        let Some(requested) = case.requested else {
            // A session that asked for no floor has none to enforce: the token it was
            // just handed is usable at once, and the payload is empty because the
            // version it already holds is the one deployed.
            let immediate = emulator.get_latest_configuration(&next).await;
            assert_eq!(
                immediate.status,
                200,
                "case {name}: a session with no negotiated floor may poll at once, got {} body {}",
                immediate.status,
                immediate.text()
            );
            assert!(
                immediate.body.is_empty(),
                "case {name}: an unchanged configuration serves an empty payload, got {:?}",
                immediate.body
            );
            continue;
        };
        let interval = Duration::from_secs(u64::from(requested.unsigned_abs()));

        // Presented at once, and again straight after: both refused, and the second
        // refusal proves the first changed nothing.
        for attempt in ["the first early presentation", "the second early presentation"] {
            let early = emulator.get_latest_configuration(&next).await;
            assert_too_early(&early, name, attempt);
            assert_token_usable(&emulator, &next, name, attempt);
        }

        // Still inside the interval, with almost all of it behind: an early poll is
        // early however little of the interval is left.
        let inside = "inside the last of the interval";
        set_poll_instant(&emulator, &next, interval - MARGIN);
        let short = emulator.get_latest_configuration(&next).await;
        assert_too_early(&short, name, inside);
        assert_token_usable(&emulator, &next, name, inside);

        // The interval has now elapsed, so the SAME token is accepted: an empty payload,
        // because the session already holds the deployed version, and a token rotated
        // again.
        set_poll_instant(&emulator, &next, interval);
        let accepted = emulator.get_latest_configuration(&next).await;
        assert_eq!(
            accepted.status,
            200,
            "case {name}: the same token must be accepted once the interval has passed, \
             got {} body {}",
            accepted.status,
            accepted.text()
        );
        assert!(
            accepted.body.is_empty(),
            "case {name}: an unchanged configuration serves an empty payload, got {:?}",
            accepted.body
        );
        let rotated = accepted.next_token();
        assert_ne!(
            rotated, next,
            "case {name}: the accepted poll must rotate the token again, got {rotated} back"
        );
        assert_eq!(
            accepted.header("next-poll-interval-in-seconds"),
            Some(reported.as_str()),
            "case {name}: the accepted poll must report the obligation again, got {:?}",
            accepted.header("next-poll-interval-in-seconds")
        );

        // The obligation belongs to the token, not to the configuration: a brand-new
        // session over the same triple is served at once, however long the interval the
        // first session negotiated.
        let fresh = start_session(&emulator, &hierarchy, case.requested).await;
        let served = emulator.get_latest_configuration(&fresh).await;
        assert_eq!(
            served.status,
            200,
            "case {name}: a new session must be served with no wait, got {} body {}",
            served.status,
            served.text()
        );
        assert_eq!(
            served.body.as_ref(),
            case.content,
            "case {name}: a new session must be served the deployed bytes, got {:?}",
            served.body
        );
    }
}

/// The whole refusal AppConfigData owes a poll inside its interval: HTTP 400, the
/// modelled error code and reason, the service's own sentence for the problem, the
/// structured discriminator a backing-off client branches on — and no configuration and
/// no rotated token, because a refusal serves nothing.
fn assert_too_early(reply: &Reply, name: &str, attempt: &str) {
    assert_eq!(
        reply.status,
        400,
        "case {name}: {attempt} expected 400, got {} body {}",
        reply.status,
        reply.text()
    );
    let body = reply.json();
    assert_eq!(
        body["__type"].as_str(),
        Some("BadRequestException"),
        "case {name}: {attempt} expected BadRequestException, got {}",
        reply.text()
    );
    assert_eq!(
        body["Message"].as_str(),
        Some("Request too early"),
        "case {name}: {attempt} expected the service's too-early sentence, got {}",
        reply.text()
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("InvalidParameters"),
        "case {name}: {attempt} expected the modelled Reason, got {}",
        reply.text()
    );
    assert_eq!(
        body["Details"]["InvalidParameters"]["ConfigurationToken"]["Problem"].as_str(),
        Some("PollIntervalNotSatisfied"),
        "case {name}: {attempt} expected the modelled problem, got {}",
        reply.text()
    );
    assert_eq!(
        reply.header("next-poll-configuration-token"),
        None,
        "case {name}: {attempt} must rotate no token, got {:?}",
        reply.header("next-poll-configuration-token")
    );
}

/// A refused poll leaves the token it refused usable: the entry is still there and has
/// not been marked as having served a call.
fn assert_token_usable(emulator: &Emulator, token: &str, name: &str, attempt: &str) {
    let state = emulator.state();
    let served = state
        .lock()
        .sessions
        .get(token)
        .map(|session| session.has_served_a_poll());
    assert_eq!(
        served,
        Some(false),
        "case {name}: {attempt} must leave the token usable, store says {served:?}"
    );
}

/// Put the instant a token was minted at `ago` into the past, as if that much of its
/// interval had passed.
///
/// The passage of time is the one input no operation takes, and the interval under test
/// is up to a day long, so the stored instant is moved instead of waited for: the answer
/// the handler computes from it is the same either way. The instant is SET rather than
/// shifted, so what the next poll sees does not depend on how long the requests before
/// it took.
fn set_poll_instant(emulator: &Emulator, token: &str, ago: Duration) {
    let state = emulator.state();
    let mut store = state.lock();
    let session = store
        .sessions
        .get_mut(token)
        .expect("the session was persisted under its token");
    assert!(
        session.last_polled_at.is_some(),
        "the poll that minted this token must have recorded its instant"
    );
    session.last_polled_at = Some(SystemTime::now() - ago);
}

/// Start a session over `hierarchy`, asking for `requested` as the minimum poll
/// interval, and answer its initial token.
async fn start_session(
    emulator: &Emulator,
    hierarchy: &Hierarchy,
    requested: Option<i32>,
) -> String {
    let minimum = requested
        .map(|seconds| format!(r#","RequiredMinimumPollIntervalInSeconds":{seconds}"#))
        .unwrap_or_default();
    let started = emulator
        .post_json(
            "/configurationsessions",
            &format!(
                r#"{{"ApplicationIdentifier":"{application}",
                      "EnvironmentIdentifier":"{environment}",
                      "ConfigurationProfileIdentifier":"{profile}"{minimum}}}"#,
                application = hierarchy.application_name,
                environment = hierarchy.environment_name,
                profile = hierarchy.profile_name,
            ),
        )
        .await;
    assert_eq!(
        started.status,
        201,
        "StartConfigurationSession({requested:?}): expected 201, got {} body {}",
        started.status,
        started.text()
    );
    started.string("InitialConfigurationToken")
}

/// Create a hierarchy, write the case's configuration into it and deploy that version
/// through a caller-created deployment strategy, exactly as a client sets a
/// configuration up — then ask the control plane for the deployment's state, so the data
/// plane is only polled for a deployment AppConfig reports finished.
async fn deploy_through_a_custom_strategy(emulator: &Emulator, case: &PollRateCase) -> Hierarchy {
    let hierarchy = emulator
        .create_hierarchy("MyApp", "Prod", "MyProfile")
        .await;
    let application_id = &hierarchy.application_id;
    let environment_id = &hierarchy.environment_id;
    let profile_id = &hierarchy.profile_id;

    let strategy = emulator
        .post_json("/deploymentstrategies", INSTANT_STRATEGY)
        .await;
    assert_eq!(
        strategy.status,
        201,
        "CreateDeploymentStrategy: expected 201, got {} body {}",
        strategy.status,
        strategy.text()
    );
    let strategy_id = strategy.string("Id");

    let written = emulator
        .send(
            "POST",
            &format!(
                "/applications/{application_id}/configurationprofiles/{profile_id}\
                 /hostedconfigurationversions"
            ),
            case.content_type,
            &[],
            Bytes::copy_from_slice(case.content),
        )
        .await;
    assert_eq!(
        written.status,
        201,
        "CreateHostedConfigurationVersion: expected 201, got {} body {}",
        written.status,
        written.text()
    );
    let version = written
        .header("version-number")
        .unwrap_or_else(|| panic!("no Version-Number header; headers {:?}", written.headers))
        .to_string();

    let started = emulator
        .post_json(
            &format!("/applications/{application_id}/environments/{environment_id}/deployments"),
            &format!(
                r#"{{"DeploymentStrategyId":"{strategy_id}",
                      "ConfigurationProfileId":"{profile_id}",
                      "ConfigurationVersion":"{version}"}}"#
            ),
        )
        .await;
    assert_eq!(
        started.status,
        201,
        "StartDeployment(version {version}): expected 201, got {} body {}",
        started.status,
        started.text()
    );
    let number = started.json()["DeploymentNumber"]
        .as_i64()
        .unwrap_or_else(|| panic!("no DeploymentNumber in {}", started.text()));

    // Every stage of a deployment after the one the start answers is owed by the CLOCK —
    // the rollout reaching its targets, and the completion that follows — so the stages
    // are let fall due (`Emulator::settle_deployments`) rather than slept for.
    emulator.settle_deployments(&hierarchy);

    let deployment = emulator
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
        deployment.status,
        200,
        "GetDeployment({number}): expected 200, got {} body {}",
        deployment.status,
        deployment.text()
    );
    assert_eq!(
        deployment.string("State"),
        DEPLOYMENT_COMPLETE,
        "case {case}: the control plane must report the deployment finished, got {}",
        deployment.text(),
        case = case.name,
    );
    hierarchy
}
