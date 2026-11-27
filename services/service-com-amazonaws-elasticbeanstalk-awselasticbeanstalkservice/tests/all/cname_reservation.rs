// SPDX-License-Identifier: CC0-1.0

//! What a `SwapEnvironmentCNAMEs` leaves `CheckDNSAvailability` reporting, over
//! this service's awsQuery surface.
//!
//! RULE: an environment HOLDS the CNAME its `CNAMEPrefix` forms from the moment
//! it is created — it reserved that name — and `CheckDNSAvailability` reports a
//! prefix as unavailable exactly while a live environment holds it, whether it is
//! the environment that reserved the name or the one answering at it now. A swap
//! exchanges the two environments' CNAMEs and takes neither name out of use, so
//! both exchanged prefixes go on reporting unavailable, and a prefix that is not
//! available reserves nothing — the report carries no `FullyQualifiedCNAME` for
//! it, since that name belongs to whoever holds it. Swapping back changes none of
//! that. A prefix is free again only once the environment holding it has
//! terminated.
//!
//! The rows span what the rule generalises over: which member the swap selects
//! each side by (id or name), which of the two prefixes is asked about, and a
//! prefix no swap touched.

use crate::lifecycle::{assert_published_cname, wait_for_status, Emulator, Answer, SOLUTION_STACK, TEST_REGION};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application every case is created under.
const APPLICATION: &str = "cname-app";

/// How a swap names the pair of environments it exchanges.
#[derive(Debug, Clone, Copy)]
enum SelectBy {
    Id,
    Name,
}

/// One row: how the pair is selected, and the two environments it swaps.
struct Row {
    case: &'static str,
    select_by: SelectBy,
    source: &'static str,
    destination: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a pair selected by environment id",
            select_by: SelectBy::Id,
            source: "blue-by-id",
            destination: "green-by-id",
        },
        Row {
            case: "a pair selected by environment name",
            select_by: SelectBy::Name,
            source: "blue-by-name",
            destination: "green-by-name",
        },
    ]
}

/// Create the environment `name` holding the CNAME `prefix` forms, wait for the
/// provisioning that assigns it that name, and answer its `EnvironmentId` and the
/// `CNAME` it publishes — held to its form, since only the service can mint the
/// sub-domain that name carries.
async fn create(emulator: &Emulator, case: &str, name: &str, prefix: &str) -> (String, String) {
    let created = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", name),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("CNAMEPrefix", prefix),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        created.status, created.body
    );
    let environment_id = created.expect("EnvironmentId", case).to_string();
    // The launch RESERVED the name and is assigned it when the provisioning it
    // started finishes, so the published CNAME is read once the environment is
    // available.
    wait_for_status(emulator, case, name, eb::state::STATUS_READY).await;
    let cname = reported_cname(emulator, case, &environment_id).await;
    assert_published_cname(case, &cname, TEST_REGION, prefix);
    (environment_id, cname)
}

/// What `CheckDNSAvailability` answers for `prefix`.
async fn checked(emulator: &Emulator, case: &str, prefix: &str) -> Answer {
    let answer = emulator
        .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from CheckDNSAvailability, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// Assert that `prefix` reports UNAVAILABLE and that the report reserves no name
/// for this caller, which is the pair the rule states together.
async fn assert_held(emulator: &Emulator, case: &str, prefix: &str, why: &str) {
    let answer = checked(emulator, case, prefix).await;
    assert_eq!(
        answer.expect("Available", case),
        "false",
        "case {case:?}: {why} ({prefix}), got {}",
        answer.body
    );
    assert!(
        answer.value("FullyQualifiedCNAME").is_none(),
        "case {case:?}: a prefix that is not available reserves no name ({prefix}), got {}",
        answer.body
    );
}

/// The CNAME `DescribeEnvironments` reports for the environment `environment_id`.
async fn reported_cname(emulator: &Emulator, case: &str, environment_id: &str) -> String {
    let described = emulator
        .call(
            "DescribeEnvironments",
            &[("EnvironmentIds.member.1", environment_id)],
        )
        .await;
    described
        .expect("Environments/member/CNAME", case)
        .to_string()
}

/// Swap the pair of environments `row` names.
async fn swap(emulator: &Emulator, row: &Row, source_id: &str, destination_id: &str) {
    let params: Vec<(&str, &str)> = match row.select_by {
        SelectBy::Id => vec![
            ("SourceEnvironmentId", source_id),
            ("DestinationEnvironmentId", destination_id),
        ],
        SelectBy::Name => vec![
            ("SourceEnvironmentName", row.source),
            ("DestinationEnvironmentName", row.destination),
        ],
    };
    let swapped = emulator.call("SwapEnvironmentCNAMEs", &params).await;
    assert_eq!(
        swapped.status, 200,
        "case {:?}: expected 200 from SwapEnvironmentCNAMEs, got {} and body {}",
        row.case, swapped.status, swapped.body
    );
}

#[tokio::test]
async fn a_swap_leaves_both_exchanged_prefixes_held_and_a_second_swap_too() {
    let emulator = Emulator::new();
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );

    for row in rows() {
        let case = row.case;
        let source_prefix = format!("{}-prefix", row.source);
        let destination_prefix = format!("{}-prefix", row.destination);
        let (source_id, source_cname) = create(&emulator, case, row.source, &source_prefix).await;
        let (destination_id, destination_cname) =
            create(&emulator, case, row.destination, &destination_prefix).await;
        // An untouched prefix, so what the swap changes is asserted against a
        // name no swap moved.
        let untouched_prefix = format!("{}-untouched", row.source);
        let (untouched_id, untouched_cname) = create(
            &emulator,
            case,
            &format!("{}-untouched", row.source),
            &untouched_prefix,
        )
        .await;

        // Each environment holds the name its own prefix formed, so none of the
        // three prefixes is available before the swap.
        for prefix in [&source_prefix, &destination_prefix, &untouched_prefix] {
            assert_held(
                &emulator,
                case,
                prefix,
                "a prefix the environment that took it holds must report unavailable",
            )
            .await;
        }

        swap(&emulator, &row, &source_id, &destination_id).await;

        // Each environment now answers at the OTHER's name...
        assert_eq!(
            reported_cname(&emulator, case, &source_id).await,
            destination_cname,
            "case {case:?}: the swap must hand the source the destination's CNAME"
        );
        assert_eq!(
            reported_cname(&emulator, case, &destination_id).await,
            source_cname,
            "case {case:?}: the swap must hand the destination the source's CNAME"
        );
        // ... and both exchanged names are still in use, by the environment that
        // reserved each and by the one now answering at it, so neither prefix
        // becomes available.
        for prefix in [&source_prefix, &destination_prefix] {
            assert_held(
                &emulator,
                case,
                prefix,
                "a swapped prefix is still in use, so it must report unavailable",
            )
            .await;
        }
        // The prefix no swap touched is still held by the environment that took
        // it.
        assert_held(
            &emulator,
            case,
            &untouched_prefix,
            "a prefix no swap touched must stay held",
        )
        .await;
        assert_eq!(
            reported_cname(&emulator, case, &untouched_id).await,
            untouched_cname,
            "case {case:?}: the untouched environment keeps its own CNAME"
        );

        // A name a live environment holds cannot be taken by a new environment:
        // two environments cannot answer at one host name.
        let taken = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", APPLICATION),
                    ("EnvironmentName", &format!("{}-intruder", row.source)),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                    ("CNAMEPrefix", source_prefix.as_str()),
                ],
            )
            .await;
        assert_eq!(
            taken.status, 400,
            "case {case:?}: expected the create to refuse a held name, got {} and body {}",
            taken.status, taken.body
        );
        // The name the refusal quotes is the one the environment holding that
        // prefix carries, which the swap handed it.
        let expected = format!("DNS name ({source_cname}) is not available.");
        assert_eq!(
            taken.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            taken.error_message()
        );

        // The swap is its own inverse, so swapping back restores both addresses
        // and changes nothing about what is available.
        swap(&emulator, &row, &source_id, &destination_id).await;
        assert_eq!(
            reported_cname(&emulator, case, &source_id).await,
            source_cname,
            "case {case:?}: swapping twice must restore the source's CNAME"
        );
        for prefix in [&source_prefix, &destination_prefix] {
            assert_held(
                &emulator,
                case,
                prefix,
                "a prefix whose environment carries it again must report unavailable",
            )
            .await;
        }
    }
}

/// Checking a prefix a swap moved neither takes it nor gives it away: a report
/// changes nothing a later caller sees.
#[tokio::test]
async fn checking_a_swapped_prefix_changes_nothing() {
    let emulator = Emulator::new();
    let case = "a swapped prefix read twice";
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
    let (source_id, blue_cname) = create(&emulator, case, "blue", "blue-prefix").await;
    let (destination_id, green_cname) = create(&emulator, case, "green", "green-prefix").await;
    let swapped = emulator
        .call(
            "SwapEnvironmentCNAMEs",
            &[
                ("SourceEnvironmentId", source_id.as_str()),
                ("DestinationEnvironmentId", destination_id.as_str()),
            ],
        )
        .await;
    assert_eq!(
        swapped.status, 200,
        "case {case:?}: expected 200 from SwapEnvironmentCNAMEs, got {} and body {}",
        swapped.status, swapped.body
    );

    for round in 0..3 {
        assert_held(
            &emulator,
            case,
            "blue-prefix",
            &format!("round {round} must still report the swapped name unavailable"),
        )
        .await;
    }
    // And the environments are exactly as the swap left them.
    assert_eq!(
        reported_cname(&emulator, case, &source_id).await,
        green_cname,
        "case {case:?}: the reads must not move either CNAME"
    );
    assert_eq!(
        reported_cname(&emulator, case, &destination_id).await,
        blue_cname,
        "case {case:?}: the reads must not move either CNAME"
    );
}
