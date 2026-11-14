// SPDX-License-Identifier: CC0-1.0

//! The CONFIGURATION a cluster carries that this fix stood up: its deferred
//! maintenance window, the snapshot schedule its backups are taken on, and
//! turning its audit logging off again — end to end over the awsQuery wire.
//!
//! The RULES these cases assert, each over a table rather than over one recorded
//! request:
//!
//! * a deferral is described by a start and EITHER an end time or a duration in
//!   days, never both and never neither, and the duration is bounded by the
//!   allowance the account publishes; what a deferral writes is the cluster's own
//!   `DeferredMaintenanceWindows`, which a later read reports;
//! * a cluster is associated with a snapshot schedule this account holds, or
//!   released from the one it has: the association is the cluster's own member, so
//!   the cluster's read and the schedule's read cannot disagree about it;
//! * `DisableLogging` stops exactly what the request named — the log types it
//!   lists, or the delivery itself when it lists none — and the record it leaves
//!   is the one `DescribeLoggingStatus` reports.
//!
//! Every wait is a poll for the CONDITION through the shared harness, never a
//! sleep. The harness is shared with [`crate::cluster_lifecycle`], which owns it.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// The parameters one row of a table sends beside the cluster it names.
type Params = Vec<(&'static str, &'static str)>;

/// One row of the deferral table: the case, the deferral's parameters and the
/// identifier the window must carry (`None` when the service mints one).
type DeferralRow = (&'static str, Params, Option<&'static str>);

/// One row of the refusal table: the case, the parameters, and the status and
/// code the request is refused with.
type RefusalRow = (&'static str, Params, u16, &'static str);

/// Create a cluster and wait until it is serving: a cluster the service is still
/// working on takes no configuration change.
async fn serving_cluster(emulator: &Emulator, case: &str, identifier: &str) {
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateCluster {identifier} expected 200, got {} with body {}",
        created.status, created.body
    );
    emulator
        .poll_until(
            case,
            "DescribeClusters",
            &[("ClusterIdentifier", identifier)],
            |answer| answer.text("ClusterStatus").as_deref() == Some(state::STATUS_AVAILABLE),
        )
        .await;
}

/// Assert one refusal: the status and the `<Code>` the model gives the fault.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// A deferral is written onto the cluster and reported back by a later read, for a
/// table spanning the two ways of saying how long it lasts and whether the request
/// brought its own identifier.
#[tokio::test]
async fn a_deferred_maintenance_window_is_written_and_read_back() {
    let emulator = Emulator::new();
    let identifier = "deferred-maintenance-cluster";
    serving_cluster(&emulator, "a deferral is written", identifier).await;

    // (case, the deferral's parameters, the identifier the window must carry or
    // `None` when the service mints one)
    let rows: Vec<DeferralRow> = vec![
        (
            "a duration in days, with no identifier of its own",
            vec![("DeferMaintenance", "true"), ("DeferMaintenanceDuration", "7")],
            None,
        ),
        (
            "the documented maximum duration",
            vec![
                ("DeferMaintenance", "true"),
                ("DeferMaintenanceDuration", "60"),
            ],
            None,
        ),
        (
            "an end time instead of a duration",
            vec![
                ("DeferMaintenance", "true"),
                ("DeferMaintenanceEndTime", "2030-01-31T00:00:00Z"),
            ],
            None,
        ),
        (
            "an identifier the request brought",
            vec![
                ("DeferMaintenance", "true"),
                ("DeferMaintenanceIdentifier", "dfm-request-chosen"),
                ("DeferMaintenanceDuration", "1"),
            ],
            Some("dfm-request-chosen"),
        ),
    ];

    for (case, parameters, wanted_identifier) in rows {
        let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", identifier)];
        params.extend(parameters.iter().copied());
        let answer = emulator.call("ModifyClusterMaintenance", &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );

        // A cluster carries ONE deferral, whatever it was described by, and the
        // window is reported with both of its instants.
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
            .await;
        let identifiers = all_text(&described.body, "DeferMaintenanceIdentifier");
        assert_eq!(
            identifiers.len(),
            1,
            "case {case}: expected exactly one deferred maintenance window, got {identifiers:?}"
        );
        if let Some(wanted) = wanted_identifier {
            assert_eq!(
                identifiers.first().map(String::as_str),
                Some(wanted),
                "case {case}: expected the identifier the request brought, got {identifiers:?}"
            );
        } else {
            assert!(
                identifiers
                    .first()
                    .is_some_and(|minted| !minted.trim().is_empty()),
                "case {case}: expected a minted identifier, got {identifiers:?}"
            );
        }
        assert_eq!(
            all_text(&described.body, "DeferMaintenanceStartTime").len(),
            1,
            "case {case}: expected the window's start reported, got body {}",
            described.body
        );
        assert_eq!(
            all_text(&described.body, "DeferMaintenanceEndTime").len(),
            1,
            "case {case}: expected the window's end reported, got body {}",
            described.body
        );
    }

    // Taking the deferral off leaves the cluster with no window at all, which is
    // what the empty group reports.
    let released = emulator
        .call(
            "ModifyClusterMaintenance",
            &[
                ("ClusterIdentifier", identifier),
                ("DeferMaintenance", "false"),
            ],
        )
        .await;
    assert_eq!(
        released.status, 200,
        "case the deferral is taken off: expected 200, got {} with body {}",
        released.status, released.body
    );
    let described = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
        .await;
    assert!(
        all_text(&described.body, "DeferMaintenanceIdentifier").is_empty(),
        "case the deferral is taken off: expected no deferred maintenance window, got body {}",
        described.body
    );
}

/// A deferral this service cannot honour is refused, and the cluster keeps the
/// window it had: both members together, neither of them, and a duration past the
/// published allowance.
#[tokio::test]
async fn a_deferral_this_service_cannot_honour_is_refused() {
    let emulator = Emulator::new();
    let identifier = "refused-deferral-cluster";
    serving_cluster(&emulator, "a refused deferral", identifier).await;

    // (case, the parameters, the status and code expected)
    let rows: Vec<RefusalRow> = vec![
        (
            "both an end time and a duration",
            vec![
                ("DeferMaintenance", "true"),
                ("DeferMaintenanceDuration", "7"),
                ("DeferMaintenanceEndTime", "2030-01-31T00:00:00Z"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "neither an end time nor a duration",
            vec![("DeferMaintenance", "true")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "one day past the published allowance",
            vec![
                ("DeferMaintenance", "true"),
                ("DeferMaintenanceDuration", "61"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a duration of no days at all",
            vec![
                ("DeferMaintenance", "true"),
                ("DeferMaintenanceDuration", "0"),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, parameters, status, code) in rows {
        let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", identifier)];
        params.extend(parameters.iter().copied());
        let answer = emulator.call("ModifyClusterMaintenance", &params).await;
        assert_refused(case, &answer, status, code);
        // Nothing was written: the cluster still carries no deferral.
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
            .await;
        assert!(
            all_text(&described.body, "DeferMaintenanceIdentifier").is_empty(),
            "case {case}: expected the refused deferral to write nothing, got body {}",
            described.body
        );
    }

    // The allowance the refusal names is the one the account publishes, so a
    // caller reads the same number from `DescribeAccountAttributes`.
    let published = emulator.call("DescribeAccountAttributes", &[]).await;
    assert!(
        published
            .body
            .contains(&state::MAX_DEFER_MAINTENANCE_DURATION.to_string()),
        "case the published allowance: expected {} among the account attributes, got body {}",
        state::MAX_DEFER_MAINTENANCE_DURATION,
        published.body
    );
}

/// A cluster is associated with a snapshot schedule this account holds and
/// released from it again; the schedule's own read reports the association from
/// the cluster, so the two cannot disagree.
#[tokio::test]
async fn a_cluster_is_associated_with_a_snapshot_schedule_and_released_from_it() {
    let emulator = Emulator::new();
    let identifier = "scheduled-cluster";
    serving_cluster(&emulator, "a cluster takes a schedule", identifier).await;

    let schedule = "nightly-schedule";
    let created = emulator
        .call(
            "CreateSnapshotSchedule",
            &[
                ("ScheduleIdentifier", schedule),
                ("ScheduleDefinitions.member.1", "rate(12 hours)"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case seed {schedule}: expected 200, got {} with body {}",
        created.status, created.body
    );

    // A schedule this account does not hold cannot be associated, whatever the
    // spelling: the schedule's own modelled fault says so.
    for (case, named) in [
        ("a schedule nothing created", "no-such-schedule"),
        ("a stored schedule in another case", "NIGHTLY-SCHEDULE"),
    ] {
        let refused = emulator
            .call(
                "ModifyClusterSnapshotSchedule",
                &[
                    ("ClusterIdentifier", identifier),
                    ("ScheduleIdentifier", named),
                ],
            )
            .await;
        assert_refused(case, &refused, 400, "SnapshotScheduleNotFound");
    }

    // A request that asks for no change at all names nothing to do.
    let silent = emulator
        .call(
            "ModifyClusterSnapshotSchedule",
            &[("ClusterIdentifier", identifier)],
        )
        .await;
    assert_refused(
        "a request naming neither a schedule nor a release",
        &silent,
        400,
        "InvalidParameterCombination",
    );

    // Releasing a cluster from a schedule it is not on is refused on the
    // association's own state.
    let not_associated = emulator
        .call(
            "ModifyClusterSnapshotSchedule",
            &[
                ("ClusterIdentifier", identifier),
                ("DisassociateSchedule", "true"),
            ],
        )
        .await;
    assert_refused(
        "a cluster associated with no schedule",
        &not_associated,
        400,
        "InvalidClusterSnapshotScheduleState",
    );

    // The association itself: the cluster reports the schedule and its state, and
    // the schedule reports the cluster.
    let associated = emulator
        .call(
            "ModifyClusterSnapshotSchedule",
            &[
                ("ClusterIdentifier", identifier),
                ("ScheduleIdentifier", schedule),
            ],
        )
        .await;
    assert_eq!(
        associated.status, 200,
        "case the association: expected 200, got {} with body {}",
        associated.status, associated.body
    );
    let described = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        described.text("SnapshotScheduleIdentifier").as_deref(),
        Some(schedule),
        "case the association: expected the cluster to report the schedule, got {:?}",
        described.text("SnapshotScheduleIdentifier")
    );
    assert_eq!(
        described.text("SnapshotScheduleState").as_deref(),
        Some(state::SCHEDULE_ASSOCIATION_STATE_ACTIVE),
        "case the association: expected SnapshotScheduleState={}, got {:?}",
        state::SCHEDULE_ASSOCIATION_STATE_ACTIVE,
        described.text("SnapshotScheduleState")
    );
    let schedules = emulator
        .call(
            "DescribeSnapshotSchedules",
            &[("ScheduleIdentifier", schedule)],
        )
        .await;
    assert!(
        all_text(&schedules.body, "ClusterIdentifier").contains(&identifier.to_string()),
        "case the association: expected the schedule to report the cluster, got body {}",
        schedules.body
    );

    // … and the release takes it off both reads.
    let released = emulator
        .call(
            "ModifyClusterSnapshotSchedule",
            &[
                ("ClusterIdentifier", identifier),
                ("DisassociateSchedule", "true"),
            ],
        )
        .await;
    assert_eq!(
        released.status, 200,
        "case the release: expected 200, got {} with body {}",
        released.status, released.body
    );
    let described = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        described.text("SnapshotScheduleIdentifier"),
        None,
        "case the release: expected the cluster to report no schedule, got {:?}",
        described.text("SnapshotScheduleIdentifier")
    );
    let schedules = emulator
        .call(
            "DescribeSnapshotSchedules",
            &[("ScheduleIdentifier", schedule)],
        )
        .await;
    assert!(
        !all_text(&schedules.body, "ClusterIdentifier").contains(&identifier.to_string()),
        "case the release: expected the schedule to report no cluster, got body {}",
        schedules.body
    );
}

/// `DisableLogging` stops exactly what the request named, and the record it leaves
/// is the one a later `DescribeLoggingStatus` reports.
#[tokio::test]
async fn disabling_logging_stops_what_the_request_named() {
    let emulator = Emulator::new();
    let identifier = "logging-cluster";
    serving_cluster(&emulator, "logging is stopped", identifier).await;

    // The delivery this suite configures names no bucket: `BucketName` and
    // `S3KeyPrefix` describe the Amazon S3 delivery alone, so a CloudWatch
    // delivery is configured by its destination and its log types — which is what
    // `DisableLogging` then stops.
    let enable = |exports: Vec<(&'static str, &'static str)>| {
        let mut params: Vec<(&'static str, &'static str)> = vec![
            ("LogDestinationType", state::LOG_DESTINATION_CLOUDWATCH),
        ];
        params.extend(exports);
        params
    };

    // One log type of several is stopped: the delivery stays on and keeps the
    // rest.
    let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", identifier)];
    params.extend(enable(vec![
        ("LogExports.member.1", "connectionlog"),
        ("LogExports.member.2", "userlog"),
    ]));
    let enabled = emulator.call("EnableLogging", &params).await;
    assert_eq!(
        enabled.status, 200,
        "case seed the delivery: expected 200, got {} with body {}",
        enabled.status, enabled.body
    );

    let narrowed = emulator
        .call(
            "DisableLogging",
            &[
                ("ClusterIdentifier", identifier),
                ("LogExports.member.1", "connectionlog"),
            ],
        )
        .await;
    assert_eq!(
        narrowed.status, 200,
        "case one log type is stopped: expected 200, got {} with body {}",
        narrowed.status, narrowed.body
    );
    let read_back = emulator
        .call("DescribeLoggingStatus", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        read_back.text("LoggingEnabled").as_deref(),
        Some("true"),
        "case one log type is stopped: expected the delivery still on, got body {}",
        read_back.body
    );
    assert_eq!(
        all_text(&read_back.body, "member"),
        vec!["userlog".to_string()],
        "case one log type is stopped: expected only the remaining log type, got body {}",
        read_back.body
    );

    // A request that names no log type stops the delivery itself, and the cluster
    // then reports exactly what a cluster nobody configured reports.
    let stopped = emulator
        .call("DisableLogging", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        stopped.status, 200,
        "case the delivery is stopped: expected 200, got {} with body {}",
        stopped.status, stopped.body
    );
    assert_eq!(
        stopped.text("LoggingEnabled").as_deref(),
        Some("false"),
        "case the delivery is stopped: expected LoggingEnabled=false, got body {}",
        stopped.body
    );
    let read_back = emulator
        .call("DescribeLoggingStatus", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        (
            read_back.text("LoggingEnabled").as_deref(),
            read_back.text("LogDestinationType"),
            read_back.text("BucketName"),
        ),
        (Some("false"), None, None),
        "case the delivery is stopped: expected logging off and nothing else reported, got \
         body {}",
        read_back.body
    );

    // Stopping the last log type is stopping the delivery: a request that names
    // every configured type leaves nothing to export.
    let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", identifier)];
    params.extend(enable(vec![("LogExports.member.1", "useractivitylog")]));
    let enabled = emulator.call("EnableLogging", &params).await;
    assert_eq!(
        enabled.status, 200,
        "case the last log type: expected 200, got {} with body {}",
        enabled.status, enabled.body
    );
    let emptied = emulator
        .call(
            "DisableLogging",
            &[
                ("ClusterIdentifier", identifier),
                ("LogExports.member.1", "useractivitylog"),
            ],
        )
        .await;
    assert_eq!(
        emptied.text("LoggingEnabled").as_deref(),
        Some("false"),
        "case the last log type: expected the delivery off once nothing is exported, got body {}",
        emptied.body
    );
}
