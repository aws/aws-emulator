// SPDX-License-Identifier: CC0-1.0

//! Reusing an environment NAME, over the service's awsQuery HTTP surface.
//!
//! The rule under test: an `EnvironmentName` "must be unique within a region in
//! your account" while an environment holds it — a second create naming it is
//! refused synchronously with `InvalidParameterValue` and the sentence the
//! service words that with — and the name is free again once the environment
//! holding it has reached `Terminated` ("Environment is not running"), where the
//! identical create succeeds with a NEW `EnvironmentId`. A client that walks that
//! sequence reads each status back off `DescribeEnvironments`, which reports the
//! records NEWEST first, so the record it reads for a name it has just re-created
//! is the one it just created rather than the terminated one it replaced.
//!
//! The table spans what the rule generalises over: which identifier the
//! terminate and the reads name the environment by, and whether the re-create
//! reuses the CNAME prefix the terminated environment held.
//!
//! The HTTP harness is the one `lifecycle.rs` already owns in this same test
//! binary.

use crate::lifecycle::{
    assert_published_cname, wait_for_status, Answer, Emulator, SOLUTION_STACK, TEST_REGION,
};

/// The application every environment below belongs to.
const APPLICATION: &str = "reuse-app";

/// The one name the whole sequence reuses.
const ENVIRONMENT: &str = "reuse-env-1";

/// One walk of the sequence: how the environment is addressed after it is
/// created, and whether the re-create asks for the same CNAME prefix.
struct Row {
    case: &'static str,
    by_id: bool,
    reuse_prefix: bool,
}

/// One row per thing the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        case: "addressed by name, a fresh prefix",
        by_id: false,
        reuse_prefix: false,
    },
    Row {
        case: "addressed by id, the prefix the terminated environment held",
        by_id: true,
        reuse_prefix: true,
    },
];

/// Call `action`, failing the named case unless it answered 200.
async fn ok(emulator: &Emulator, case: &str, action: &str, params: &[(&str, &str)]) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// Launch [`ENVIRONMENT`] with the CNAME prefix `prefix`, answering the whole
/// response.
async fn launch(emulator: &Emulator, case: &str, prefix: &str) -> Answer {
    ok(
        emulator,
        case,
        "CreateEnvironment",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", ENVIRONMENT),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
            ("CNAMEPrefix", prefix),
        ],
    )
    .await
}

/// The `Status` of [`ENVIRONMENT`] as a client polling it reads it: the first
/// record the default report lists for that name.
async fn polled_status(emulator: &Emulator, case: &str) -> String {
    let described = ok(
        emulator,
        case,
        "DescribeEnvironments",
        &[("EnvironmentNames.member.1", ENVIRONMENT)],
    )
    .await;
    described
        .values_at("Environments/member/Status")
        .first()
        .map(|status| (*status).to_string())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_name_is_held_until_its_environment_is_terminated_and_can_then_be_taken_again() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        ok(
            &emulator,
            case,
            "CreateApplication",
            &[("ApplicationName", APPLICATION)],
        )
        .await;

        let launched = launch(&emulator, case, "reuse-first").await;
        let first_id = launched.expect("EnvironmentId", case).to_string();
        // The launch the clock owes is waited for as the CONDITION the
        // environment reaches, never as a fixed time.
        wait_for_status(&emulator, case, ENVIRONMENT, "Ready").await;
        assert_eq!(
            polled_status(&emulator, case).await,
            "Ready",
            "case {case:?}: the launch the clock owed must complete"
        );

        // A second create naming the live environment's name is refused, with
        // the sentence the service words a name that is already taken with.
        let duplicate = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", APPLICATION),
                    ("EnvironmentName", ENVIRONMENT),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                ],
            )
            .await;
        assert_eq!(
            duplicate.status, 400,
            "case {case:?}: a duplicate name must be refused, got {} and body {}",
            duplicate.status, duplicate.body
        );
        assert_eq!(
            duplicate.error_code(),
            "InvalidParameterValue",
            "case {case:?}: unexpected error code in {}",
            duplicate.body
        );
        assert_eq!(
            duplicate.error_message(),
            format!("Environment {ENVIRONMENT} already exists."),
            "case {case:?}: unexpected refusal message in {}",
            duplicate.body
        );

        // Terminating it frees the name, once the clock has completed the
        // termination.
        let identifier: Vec<(&str, &str)> = if row.by_id {
            vec![("EnvironmentId", first_id.as_str())]
        } else {
            vec![("EnvironmentName", ENVIRONMENT)]
        };
        let terminating = ok(&emulator, case, "TerminateEnvironment", &identifier).await;
        assert_eq!(
            terminating.expect("Status", case),
            "Terminating",
            "case {case:?}: the terminate must report Terminating, got {}",
            terminating.body
        );
        wait_for_status(&emulator, case, ENVIRONMENT, "Terminated").await;
        let terminated = ok(
            &emulator,
            case,
            "DescribeEnvironments",
            &[
                ("EnvironmentNames.member.1", ENVIRONMENT),
                ("IncludeDeleted", "true"),
            ],
        )
        .await;
        assert_eq!(
            terminated.values_at("Environments/member/Status"),
            vec!["Terminated"],
            "case {case:?}: the termination the clock owed must complete, got {}",
            terminated.body
        );

        // The identical create now succeeds, with an environment id of its own.
        let prefix = if row.reuse_prefix {
            "reuse-first"
        } else {
            "reuse-second"
        };
        let again = launch(&emulator, case, prefix).await;
        let second_id = again.expect("EnvironmentId", case).to_string();
        assert_ne!(
            second_id, first_id,
            "case {case:?}: the re-created environment must carry a new EnvironmentId"
        );
        assert!(
            again.value("CNAME").is_none(),
            "case {case:?}: a launch still provisioning reports no CNAME, got {}",
            again.body
        );

        // And a client polling that NAME reads the record it has just created,
        // not the terminated one it replaced: the newest record is the first.
        assert_eq!(
            polled_status(&emulator, case).await,
            "Launching",
            "case {case:?}: the first record reported for a reused name is the new one"
        );
        wait_for_status(&emulator, case, ENVIRONMENT, "Ready").await;
        assert_eq!(
            polled_status(&emulator, case).await,
            "Ready",
            "case {case:?}: the new environment's own launch must complete"
        );
        // Once that launch has finished the environment is assigned the name it
        // reserved, under whichever prefix the re-create asked for.
        let published = ok(
            &emulator,
            case,
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", ENVIRONMENT)],
        )
        .await;
        assert_published_cname(
            case,
            published.expect("Environments/member/CNAME", case),
            TEST_REGION,
            prefix,
        );
        let both = ok(
            &emulator,
            case,
            "DescribeEnvironments",
            &[
                ("EnvironmentNames.member.1", ENVIRONMENT),
                ("IncludeDeleted", "true"),
            ],
        )
        .await;
        assert_eq!(
            both.values_at("Environments/member/EnvironmentId"),
            vec![second_id.as_str(), first_id.as_str()],
            "case {case:?}: both records are reported, newest first, got {}",
            both.body
        );
    }
}
