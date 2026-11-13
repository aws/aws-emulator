// SPDX-License-Identifier: CC0-1.0

//! `CheckDNSAvailability`, over the service's awsQuery HTTP surface.
//!
//! The rules under test: the DNS name a prefix forms is
//! `<prefix>.<region>.elasticbeanstalk.com`, which is what
//! `FullyQualifiedCNAME` reports — the name the check is ABOUT — while the
//! address the environment holding it is SERVED at is the emulator's own endpoint
//! with the environment's Region and that prefix in the path, because a client
//! reads THAT address and dials it. The name can be RESERVED by at most ONE live
//! environment in its Region — the one that took it when it was created and still
//! carries it — and it is available exactly when the prefix can be a host name
//! label at all and no live environment holds that reservation. Checking a prefix
//! is a REPORT: it reserves nothing and releases nothing. What a swap leaves of
//! those reservations is `cname_reservation.rs`.
//!
//! The HTTP harness (the router, the form encoder and the XML element reader) is
//! the one `lifecycle.rs` already owns in this same test binary, so this suite
//! adds no second copy of it.

use crate::lifecycle::{Emulator, SOLUTION_STACK, TEST_REGION};

/// How long a poll below waits for a transition the CLOCK owes to be completed before it
/// gives up: generous, so a loaded host is not what decides the outcome. The transition
/// itself is the service's own span, which is far shorter.
const RELEASE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// How long a poll below waits between two looks.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// The address `prefix` forms in `region`, as `state::environment_cname` builds
/// it: the socket this emulator serves the environment plane on, then the plane
/// path that prefix's environment is served at. Read off the emulator, because
/// the port that socket answers on is the core's to choose.
fn served_at(emulator: &Emulator, prefix: &str, region: &str) -> String {
    emulator.environment_address(region, prefix)
}

/// The DNS name `prefix` forms in `region`, as `CheckDNSAvailability` reports it:
/// `<prefix>.<region>.elasticbeanstalk.com`, spelled out here rather than read off
/// the crate so the FORM is what this suite holds the report to.
fn dns_name(prefix: &str, region: &str) -> String {
    format!("{prefix}.{region}.elasticbeanstalk.com")
}

/// One `CheckDNSAvailability` call: its prefix, the region it is signed for, and
/// whether the answer must be `true`.
struct AvailabilityCase {
    name: &'static str,
    prefix: &'static str,
    region: &'static str,
    available: bool,
}

/// A prefix exactly at the `DNSCnamePrefix` length trait's minimum (4).
const MIN_LENGTH_PREFIX: &str = "abcd";

#[tokio::test]
async fn availability_is_derived_from_the_prefix_and_the_live_environments() {
    let emulator = Emulator::new();
    // A prefix exactly at the length trait's maximum (63).
    let max_length_prefix: String = "z".repeat(63);

    let setup = emulator
        .call("CreateApplication", &[("ApplicationName", "dns")])
        .await;
    assert_eq!(
        setup.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        setup.status, setup.body
    );
    // One live environment holds exactly one CNAME: `held-prefix` in TEST_REGION.
    let held = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "dns"),
                ("EnvironmentName", "held-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("CNAMEPrefix", "held-prefix"),
            ],
        )
        .await;
    assert_eq!(
        held.status, 200,
        "case \"setup\": expected 200 from CreateEnvironment, got {} and body {}",
        held.status, held.body
    );
    assert_eq!(
        held.expect("CNAME", "setup"),
        served_at(&emulator, "held-prefix", TEST_REGION),
        "case \"setup\": the environment must hold the CNAME its prefix forms, got {}",
        held.body
    );

    // Rows differ along what the rule generalises over: whether the name is
    // held, the region the name is asked about, the case it is spelled in, the
    // length of the prefix, and the character that makes a prefix no host name
    // label at all.
    let cases = [
        AvailabilityCase {
            name: "unused prefix",
            prefix: "free-prefix",
            region: TEST_REGION,
            available: true,
        },
        AvailabilityCase {
            name: "prefix held by a live environment",
            prefix: "held-prefix",
            region: TEST_REGION,
            available: false,
        },
        AvailabilityCase {
            name: "held prefix in a different case",
            // DNS names are case-insensitive, so this names the SAME CNAME.
            prefix: "Held-Prefix",
            region: TEST_REGION,
            available: false,
        },
        AvailabilityCase {
            name: "held prefix in another region",
            // The region is part of the name, so the same prefix is a different
            // CNAME elsewhere and nothing holds that one.
            prefix: "held-prefix",
            region: "eu-central-1",
            available: true,
        },
        AvailabilityCase {
            name: "shortest legal prefix",
            prefix: MIN_LENGTH_PREFIX,
            region: TEST_REGION,
            available: true,
        },
        AvailabilityCase {
            name: "digits only",
            prefix: "12345",
            region: TEST_REGION,
            available: true,
        },
        AvailabilityCase {
            name: "underscore is not a host name character",
            prefix: "bad_prefix",
            region: TEST_REGION,
            available: false,
        },
        AvailabilityCase {
            name: "a dot would add a label",
            prefix: "bad.prefix",
            region: TEST_REGION,
            available: false,
        },
        AvailabilityCase {
            name: "leading hyphen",
            prefix: "-leading",
            region: TEST_REGION,
            available: false,
        },
        AvailabilityCase {
            name: "trailing hyphen",
            prefix: "trailing-",
            region: TEST_REGION,
            available: false,
        },
        AvailabilityCase {
            name: "space is not a host name character",
            prefix: "bad prefix",
            region: TEST_REGION,
            available: false,
        },
    ];

    for case in &cases {
        let answer = emulator
            .call_in_region(
                case.region,
                "CheckDNSAvailability",
                &[("CNAMEPrefix", case.prefix)],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected 200 from CheckDNSAvailability, got {} and body {}",
            case.name, answer.status, answer.body
        );
        assert_eq!(
            answer.expect("Available", case.name),
            if case.available { "true" } else { "false" },
            "case {:?}: unexpected Available in {}",
            case.name,
            answer.body
        );
        assert_eq!(
            answer.expect("FullyQualifiedCNAME", case.name),
            dns_name(case.prefix, case.region),
            "case {:?}: unexpected FullyQualifiedCNAME in {}",
            case.name,
            answer.body
        );
    }

    // The published length bounds of `DNSCnamePrefix` (4..63): both ends answer,
    // and one character past either end is refused before the handler runs.
    let longest = emulator
        .call(
            "CheckDNSAvailability",
            &[("CNAMEPrefix", max_length_prefix.as_str())],
        )
        .await;
    assert_eq!(
        longest.status, 200,
        "case \"longest legal prefix\": expected 200, got {} and body {}",
        longest.status, longest.body
    );
    assert_eq!(
        longest.expect("Available", "longest legal prefix"),
        "true",
        "case \"longest legal prefix\": unexpected Available in {}",
        longest.body
    );
    for (name, prefix) in [
        ("one below the minimum length", "abc".to_string()),
        ("one above the maximum length", "z".repeat(64)),
    ] {
        let refused = emulator
            .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix.as_str())])
            .await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: expected the modelled length bound to refuse the request, got {} and body {}",
            refused.status, refused.body
        );
        assert!(
            refused.value("Available").is_none(),
            "case {name:?}: a refused request must report no availability, got {}",
            refused.body
        );
    }
}

#[tokio::test]
async fn checking_a_prefix_neither_reserves_nor_releases_it() {
    let emulator = Emulator::new();
    let case = "purity";
    let prefix = "pure-prefix";

    let setup = emulator
        .call("CreateApplication", &[("ApplicationName", "dns")])
        .await;
    assert_eq!(
        setup.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        setup.status, setup.body
    );

    // Repeated checks of a free name all answer the same, and none of them takes
    // the name: the create that follows still gets it.
    for round in 0..3 {
        let answer = emulator
            .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix)])
            .await;
        assert_eq!(
            answer.expect("Available", case),
            "true",
            "case {case:?}: round {round} must still report the free name available, got {}",
            answer.body
        );
    }
    let created = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "dns"),
                ("EnvironmentName", "pure-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("CNAMEPrefix", prefix),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: checking a prefix must not reserve it, got {} and body {}",
        created.status, created.body
    );
    let environment_id = created.expect("EnvironmentId", case).to_string();

    // Now it is held, and repeated checks do not release it.
    for round in 0..3 {
        let answer = emulator
            .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix)])
            .await;
        assert_eq!(
            answer.expect("Available", case),
            "false",
            "case {case:?}: round {round} must still report the held name unavailable, got {}",
            answer.body
        );
    }

    // And the store the check reported on is untouched: the environment still
    // holds the same CNAME and is still live.
    let described = emulator
        .call(
            "DescribeEnvironments",
            &[("EnvironmentIds.member.1", environment_id.as_str())],
        )
        .await;
    assert_eq!(
        described.values_at("Environments/member/CNAME"),
        vec![served_at(&emulator, prefix, TEST_REGION).as_str()],
        "case {case:?}: the checked name must still belong to its environment, got {}",
        described.body
    );

    // A terminated environment holds nothing, so once the clock has completed
    // the termination the name is available again.
    let terminated = emulator
        .call(
            "TerminateEnvironment",
            &[("EnvironmentId", environment_id.as_str())],
        )
        .await;
    assert_eq!(
        terminated.status, 200,
        "case {case:?}: expected 200 from TerminateEnvironment, got {} and body {}",
        terminated.status, terminated.body
    );
    let while_terminating = emulator
        .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix)])
        .await;
    assert_eq!(
        while_terminating.expect("Available", case),
        "false",
        "case {case:?}: an environment still shutting down keeps its name, got {}",
        while_terminating.body
    );
    // Completing the termination is a transition the CLOCK owes, so the name becomes
    // available again on its own: look again until it is, or until a generous deadline
    // passes — what is asserted is the CONDITION, never how fast this host reached it.
    let deadline = std::time::Instant::now() + RELEASE_DEADLINE;
    let after_termination = loop {
        let answer = emulator
            .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix)])
            .await;
        if answer.expect("Available", case) == "true" || std::time::Instant::now() >= deadline {
            break answer;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        after_termination.expect("Available", case),
        "true",
        "case {case:?}: a terminated environment releases its name, got {}",
        after_termination.body
    );
}

#[tokio::test]
async fn create_environment_holds_the_same_cname_rule() {
    let emulator = Emulator::new();
    let setup = emulator
        .call("CreateApplication", &[("ApplicationName", "dns")])
        .await;
    assert_eq!(
        setup.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        setup.status, setup.body
    );
    let held = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "dns"),
                ("EnvironmentName", "taken-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("CNAMEPrefix", "taken-prefix"),
            ],
        )
        .await;
    assert_eq!(
        held.status, 200,
        "case \"setup\": expected 200 from CreateEnvironment, got {} and body {}",
        held.status, held.body
    );

    // Every prefix CheckDNSAvailability reports unavailable is also one
    // CreateEnvironment refuses: a name no live environment can hold, or one
    // another live environment already holds.
    for (name, prefix) in [
        ("taken name", "taken-prefix"),
        ("taken name in another case", "TAKEN-PREFIX"),
        ("underscore", "bad_prefix"),
        ("leading hyphen", "-leading"),
        ("trailing hyphen", "trailing-"),
    ] {
        let environment_name = format!("env-{}", name.replace(' ', "-"));
        let refused = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", "dns"),
                    ("EnvironmentName", environment_name.as_str()),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                    ("CNAMEPrefix", prefix),
                ],
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: expected CreateEnvironment to refuse the prefix, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {name:?}: unexpected error code in {}",
            refused.body
        );
        let check = emulator
            .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix)])
            .await;
        assert_eq!(
            check.expect("Available", name),
            "false",
            "case {name:?}: the check and the create must agree, got {}",
            check.body
        );
    }

    // The same prefix in another region is another name, so it is free there.
    let elsewhere = emulator
        .call_in_region(
            "eu-central-1",
            "CreateEnvironment",
            &[
                ("ApplicationName", "dns"),
                ("EnvironmentName", "taken-env-elsewhere"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("CNAMEPrefix", "taken-prefix"),
            ],
        )
        .await;
    assert_eq!(
        elsewhere.status, 200,
        "case \"another region\": the same prefix names another CNAME there, got {} and body {}",
        elsewhere.status, elsewhere.body
    );
    assert_eq!(
        elsewhere.expect("CNAME", "another region"),
        served_at(&emulator, "taken-prefix", "eu-central-1"),
        "case \"another region\": unexpected CNAME in {}",
        elsewhere.body
    );
}
