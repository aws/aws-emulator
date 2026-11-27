// SPDX-License-Identifier: CC0-1.0

//! What an environment still in its INITIAL LAUNCH reports and what can be done
//! to it, over this service's awsQuery surface.
//!
//! RULE 1 (the launch cannot be torn down mid-provision): an environment whose
//! initial launch has not got past its first phase is not one a termination can
//! take down, so `TerminateEnvironment` is refused `InvalidParameterValue` with
//! the sentence naming the environment and the phase it is in, and the
//! environment goes on launching. Once the clock has made the launch's
//! completion due — the CONDITION a client polls for — the identical call is
//! served and the environment reports `Terminating`.
//!
//! RULE 2 (an address is reported once it exists): the launch RESERVES the CNAME
//! its prefix forms, and the environment is ASSIGNED that name when the
//! provisioning the launch started has finished. So the launch's own response and
//! every read of the environment before that carry NO `CNAME` at all, and the
//! reads after it carry the name the launch reserved — the same name
//! `CheckDNSAvailability` reports the prefix as taken for from the moment of the
//! launch, since a reservation holds a name whether or not it is serving yet.
//!
//! The rows span what the rules generalise over: how the request addresses the
//! environment (by name and by id), whether the launch supplied a CNAME prefix or
//! left it to be generated, and which operation launched it
//! (`CreateEnvironment`, and `ComposeEnvironments` for the CNAME rule).

use crate::composed_environments::{compose, given_application as given_composed_application, given_version, manifest};
use crate::harness::{wait_for_status, Emulator, SOLUTION_STACK};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application the rows launch under.
const APPLICATION: &str = "launch-phase-app";

/// One row: how the termination addresses the environment, the environment it
/// launches and the CNAME prefix that launch supplies.
struct Row {
    case: &'static str,
    environment: &'static str,
    cname_prefix: Option<&'static str>,
    by_id: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "addressed by name, with a prefix of its own",
            environment: "phase-named-env",
            cname_prefix: Some("phase-named"),
            by_id: false,
        },
        Row {
            case: "addressed by id, with a generated prefix",
            environment: "phase-by-id-env",
            cname_prefix: None,
            by_id: true,
        },
    ]
}

/// Create the application the rows launch under.
async fn given_application(emulator: &Emulator, case: &str) {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
}

/// The `CNAME` `DescribeEnvironments` reports for `environment`, or `None` when it
/// publishes none.
async fn reported_cname(emulator: &Emulator, case: &str, environment: &str) -> Option<String> {
    let described = emulator
        .call(
            "DescribeEnvironments",
            &[
                ("EnvironmentNames.member.1", environment),
                ("IncludeDeleted", "true"),
            ],
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected 200 from DescribeEnvironments, got {} and body {}",
        described.status, described.body
    );
    described
        .value("Environments/member/CNAME")
        .map(str::to_string)
}

/// RULE 1, over the ways a termination addresses the environment.
#[tokio::test]
async fn a_launch_that_has_not_got_past_its_first_phase_cannot_be_terminated() {
    for row in rows() {
        let case = row.case;
        // One emulator per row, so the launch under test is the only one the
        // clock owes anything to.
        let emulator = Emulator::new();
        given_application(&emulator, case).await;

        let mut params: Vec<(&str, &str)> = vec![
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", row.environment),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
        ];
        if let Some(prefix) = row.cname_prefix {
            params.push(("CNAMEPrefix", prefix));
        }
        let launched = emulator.call("CreateEnvironment", &params).await;
        assert_eq!(
            launched.status, 200,
            "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
            launched.status, launched.body
        );
        assert_eq!(
            launched.value("Status"),
            Some(eb::state::STATUS_LAUNCHING),
            "case {case:?}: a new environment starts Launching, got body {}",
            launched.body
        );
        let environment_id = launched.expect("EnvironmentId", case).to_string();
        let identifier: Vec<(&str, &str)> = if row.by_id {
            vec![("EnvironmentId", environment_id.as_str())]
        } else {
            vec![("EnvironmentName", row.environment)]
        };

        // The termination asked for while the launch is in its first phase is
        // refused, and the environment is left launching.
        let refused = emulator.call("TerminateEnvironment", &identifier).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: unexpected error code in {}",
            refused.body
        );
        let expected = format!(
            "Cannot terminate environment named {}. It is currently {}.",
            row.environment,
            eb::state::LAUNCH_PHASE_INITIAL
        );
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
        assert!(
            refused.value("Status").is_none(),
            "case {case:?}: a refused termination reports no environment, got {}",
            refused.body
        );

        // Once the launch the clock owes has completed — waited for as a
        // CONDITION, never as a fixed time — the identical call is served.
        wait_for_status(&emulator, case, row.environment, eb::state::STATUS_READY).await;
        let served = emulator.call("TerminateEnvironment", &identifier).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: expected 200 from TerminateEnvironment once the launch is done, got \
             {} and body {}",
            served.status, served.body
        );
        assert_eq!(
            served.value("Status"),
            Some(eb::state::STATUS_TERMINATING),
            "case {case:?}: the served termination reports Terminating, got body {}",
            served.body
        );
    }
}

/// RULE 2, over the ways a launch takes its CNAME prefix.
#[tokio::test]
async fn a_launch_publishes_its_cname_once_it_has_been_assigned_one() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        given_application(&emulator, case).await;

        let mut params: Vec<(&str, &str)> = vec![
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", row.environment),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
        ];
        if let Some(prefix) = row.cname_prefix {
            params.push(("CNAMEPrefix", prefix));
        }
        let launched = emulator.call("CreateEnvironment", &params).await;
        assert_eq!(
            launched.status, 200,
            "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
            launched.status, launched.body
        );
        // The launch itself publishes no address...
        assert!(
            launched.value("CNAME").is_none(),
            "case {case:?}: a launch still provisioning reports no CNAME, got {}",
            launched.body
        );
        assert_eq!(
            reported_cname(&emulator, case, row.environment).await,
            None,
            "case {case:?}: a read of a launching environment reports no CNAME either"
        );
        // ... while the prefix it RESERVED is already taken: a second launch
        // asking for it is refused, and the check reports it unavailable.
        if let Some(prefix) = row.cname_prefix {
            let checked = emulator
                .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix)])
                .await;
            assert_eq!(
                checked.value("Available"),
                Some("false"),
                "case {case:?}: the prefix the launch reserved must report unavailable, got {}",
                checked.body
            );
            let taken = emulator
                .call(
                    "CreateEnvironment",
                    &[
                        ("ApplicationName", APPLICATION),
                        ("EnvironmentName", "phase-intruder-env"),
                        ("SolutionStackName", SOLUTION_STACK.as_str()),
                        ("CNAMEPrefix", prefix),
                    ],
                )
                .await;
            assert_eq!(
                taken.status, 400,
                "case {case:?}: a prefix a launch reserved cannot be taken, got {} and body {}",
                taken.status, taken.body
            );
        }

        // Once the provisioning has finished the environment publishes the name
        // it reserved, whose first label is that prefix.
        wait_for_status(&emulator, case, row.environment, eb::state::STATUS_READY).await;
        let cname = reported_cname(&emulator, case, row.environment)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected a CNAME once the launch is done"));
        let expected_prefix = row.cname_prefix.map(str::to_string).unwrap_or_else(|| {
            // "If not specified, the CNAME is generated automatically by
            // appending a random alphanumeric string to the environment name."
            format!("{}-", row.environment)
        });
        assert!(
            cname.starts_with(&expected_prefix),
            "case {case:?}: the assigned CNAME must carry the prefix the launch reserved \
             ({expected_prefix:?}), got {cname:?}"
        );
    }
}

/// RULE 2 through the OTHER launch: a composed environment is assigned its name
/// the same way, so the composition's own answer carries none.
#[tokio::test]
async fn a_composed_launch_publishes_its_cname_once_it_has_been_assigned_one() {
    let case = "a composed launch";
    let emulator = Emulator::with_object_storage();
    // The composition reads the versions of the application that suite's own
    // helpers create, so the group is set up through them.
    given_composed_application(&emulator, case).await;
    let stack = SOLUTION_STACK.as_str().to_string();
    let environment = "phase-composed-env";
    given_version(
        &emulator,
        case,
        "v1",
        &[(
            "env.yaml",
            &manifest(environment, Some(&stack), None, &[], &[]),
        )],
    )
    .await;

    let composed = compose(&emulator, None, &["v1"]).await;
    assert_eq!(
        composed.status, 200,
        "case {case:?}: expected 200 from ComposeEnvironments, got {} and body {}",
        composed.status, composed.body
    );
    assert_eq!(
        composed.values_at("Environments/member/EnvironmentName"),
        vec![environment],
        "case {case:?}: expected the manifest's environment, got body {}",
        composed.body
    );
    assert!(
        composed.values_at("Environments/member/CNAME").is_empty(),
        "case {case:?}: a composition still provisioning reports no CNAME, got body {}",
        composed.body
    );

    wait_for_status(&emulator, case, environment, eb::state::STATUS_READY).await;
    let cname = reported_cname(&emulator, case, environment)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: expected a CNAME once the launch is done"));
    assert!(
        cname.starts_with(environment),
        "case {case:?}: the assigned CNAME must carry this environment's own prefix, got \
         {cname:?}"
    );
}
