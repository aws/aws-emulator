// SPDX-License-Identifier: CC0-1.0

//! Changing the retention period of a cluster's cross-Region snapshot copy
//! configuration, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness and the
//! `ClusterSnapshotCopyStatus` reader are shared with
//! [`crate::cluster_lifecycle`] and [`crate::snapshot_copy_configuration`]), so
//! each assertion covers the whole walk: form-urlencoded deserialize -> handler
//! -> XML serialize.
//!
//! The RULE the suite asserts: `ModifySnapshotCopyRetentionPeriod` rewrites
//! exactly ONE retention period of the copy configuration the named cluster
//! already carries — the automated one by default, the manual one under the
//! manual option — leaving that configuration's other members and the cluster
//! itself as they stood, so the answer and every later read of the cluster
//! report the new period; and it rewrites nothing when a precondition the model
//! states fails: an absent cluster, one not in the available state, one that is
//! not copying at all, or a period outside the published range for the kind of
//! snapshot it retains.
//!
//! The tables span what the rule generalises over — which period the request
//! changes, whether the manual option is omitted, false or true, each published
//! bound of each range and values past it, and the order a sequence of changes
//! arrives in — rather than one recorded request.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};
use crate::snapshot_copy_configuration::{
    assert_refused, available_cluster, available_clusters, copy_status, create_grant, describe,
    enable, CopyStatus, GRANT,
};

/// The destination every cluster in this suite copies to.
const BASE_DESTINATION: &str = "eu-central-1";

/// The automated retention period every cluster in this suite starts on, inside
/// [`state::SNAPSHOT_COPY_RETENTION_BOUNDS`] and distinct from
/// [`state::DEFAULT_SNAPSHOT_COPY_RETENTION`], so a reported value cannot be the
/// default standing in for a period nothing wrote.
const BASE_AUTOMATED: &str = "10";

/// The manual retention period every cluster in this suite starts on, inside
/// [`state::MANUAL_RETENTION_BOUNDS`] and distinct from both the automated one
/// above and [`state::MANUAL_RETENTION_INDEFINITE`].
const BASE_MANUAL: &str = "20";

/// One row of the change table: the period the request names and whether it
/// carries the manual option.
struct ModifyCase {
    case: &'static str,
    /// `Manual`, or `None` when the request names none.
    manual: Option<&'static str>,
    /// `RetentionPeriod`.
    period: &'static str,
}

impl ModifyCase {
    /// Whether this row changes the MANUAL period: the manual option decides it,
    /// and omitting the option is the automated default.
    fn is_manual(&self) -> bool {
        self.manual == Some("true")
    }
}

/// `ModifySnapshotCopyRetentionPeriod` with the members one row carries.
async fn modify(
    emulator: &Emulator,
    cluster: &str,
    period: &str,
    manual: Option<&str>,
) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", cluster),
        ("RetentionPeriod", period),
    ];
    if let Some(manual) = manual {
        params.push(("Manual", manual));
    }
    emulator
        .call("ModifySnapshotCopyRetentionPeriod", &params)
        .await
}

/// The configuration every cluster in this suite starts on.
fn baseline() -> CopyStatus {
    CopyStatus {
        destination_region: Some(BASE_DESTINATION.to_string()),
        retention_period: Some(BASE_AUTOMATED.to_string()),
        manual_snapshot_retention_period: Some(BASE_MANUAL.to_string()),
        snapshot_copy_grant_name: Some(GRANT.to_string()),
    }
}

/// The configuration the baseline becomes once `period` is set on one of its two
/// kinds of snapshot.
fn expected(period: &str, manual: bool) -> CopyStatus {
    let mut status = baseline();
    if manual {
        status.manual_snapshot_retention_period = Some(period.to_string());
    } else {
        status.retention_period = Some(period.to_string());
    }
    status
}

/// Put one available cluster on the baseline copy configuration.
async fn start_copying(emulator: &Emulator, cluster: &str) {
    let answer = enable(
        emulator,
        cluster,
        BASE_DESTINATION,
        &[
            ("RetentionPeriod", BASE_AUTOMATED),
            ("ManualSnapshotRetentionPeriod", BASE_MANUAL),
            ("SnapshotCopyGrantName", GRANT),
        ],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {cluster}: expected the copy configured, got {} with body {}",
        answer.status, answer.body
    );
}

/// The period a request names is the one the answer and every later read report,
/// and it is the only member of the configuration that moves.
#[tokio::test]
async fn the_named_period_replaces_exactly_the_one_it_names() {
    let emulator = Emulator::new();
    create_grant(&emulator, GRANT).await;

    // The rows differ along which period the request changes, how the manual
    // option is spelled (omitted, false, true), each published bound of each
    // range, the "retained indefinitely" sentinel, and a period equal to the one
    // already in effect.
    let rows = [
        ModifyCase {
            case: "an automated period with no manual option",
            manual: None,
            period: "3",
        },
        ModifyCase {
            case: "the shortest automated period",
            manual: None,
            period: "1",
        },
        ModifyCase {
            case: "the longest automated period, manual option false",
            manual: Some("false"),
            period: "35",
        },
        ModifyCase {
            case: "the automated period it already has",
            manual: None,
            period: BASE_AUTOMATED,
        },
        ModifyCase {
            case: "the shortest manual period",
            manual: Some("true"),
            period: "1",
        },
        ModifyCase {
            case: "the longest manual period",
            manual: Some("true"),
            period: "3653",
        },
        ModifyCase {
            case: "manual snapshots retained indefinitely",
            manual: Some("true"),
            period: "-1",
        },
        ModifyCase {
            case: "the manual period it already has",
            manual: Some("true"),
            period: BASE_MANUAL,
        },
    ];

    let clusters: Vec<String> = (0..rows.len())
        .map(|index| format!("retention-cluster-{index}"))
        .collect();
    available_clusters(&emulator, &clusters).await;

    for (row, cluster) in rows.iter().zip(&clusters) {
        let case = row.case;
        start_copying(&emulator, cluster).await;

        // Before the change the cluster is on the baseline, so every member the
        // assertions below read is one this suite put there.
        let before = describe(&emulator, cluster).await;
        assert_eq!(
            copy_status(&before.body),
            Some(baseline()),
            "case {case}: expected the baseline configuration, got {:?}",
            copy_status(&before.body)
        );

        let wanted = expected(row.period, row.is_manual());
        let answer = modify(&emulator, cluster, row.period, row.manual).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the change served, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            copy_status(&answer.body).as_ref(),
            Some(&wanted),
            "case {case}: expected the answer to report {wanted:?}, got {:?}",
            copy_status(&answer.body)
        );
        // Changing a standing instruction is not work done TO the cluster, so
        // the cluster is still the available cluster it was.
        assert_eq!(
            answer.text("ClusterStatus").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {case}: expected the cluster to stay {}, got {:?}",
            state::STATUS_AVAILABLE,
            answer.text("ClusterStatus")
        );

        let read = describe(&emulator, cluster).await;
        assert_eq!(
            copy_status(&read.body).as_ref(),
            Some(&wanted),
            "case {case}: expected a later read to report {wanted:?}, got {:?}",
            copy_status(&read.body)
        );
    }
}

/// A sequence of changes on ONE cluster accumulates: each call moves its own
/// period and keeps whatever the calls before it left on the other one.
#[tokio::test]
async fn each_change_keeps_what_the_changes_before_it_left() {
    let emulator = Emulator::new();
    create_grant(&emulator, GRANT).await;
    let cluster = "accumulating-cluster";
    available_cluster(&emulator, cluster).await;
    start_copying(&emulator, cluster).await;

    // The same two periods, written in an interleaved order: whichever member a
    // call does not name has to still carry the value the previous call set.
    let mut wanted = baseline();
    for (case, period, manual) in [
        ("the first automated change", "2", None),
        ("a manual change after it", "5", Some("true")),
        ("a second automated change", "35", None),
        ("a manual change to the sentinel", "-1", Some("true")),
        ("a last automated change", "1", None),
    ] {
        if manual == Some("true") {
            wanted.manual_snapshot_retention_period = Some(period.to_string());
        } else {
            wanted.retention_period = Some(period.to_string());
        }
        let answer = modify(&emulator, cluster, period, manual).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the change served, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            copy_status(&answer.body).as_ref(),
            Some(&wanted),
            "case {case}: expected the answer to report {wanted:?}, got {:?}",
            copy_status(&answer.body)
        );
        let read = describe(&emulator, cluster).await;
        assert_eq!(
            copy_status(&read.body).as_ref(),
            Some(&wanted),
            "case {case}: expected a later read to report {wanted:?}, got {:?}",
            copy_status(&read.body)
        );
    }
}

/// A period is accepted exactly inside the published range for the kind of
/// snapshot it retains, and a value outside it changes nothing.
#[tokio::test]
async fn a_period_is_accepted_exactly_inside_the_range_for_its_kind_of_snapshot() {
    let emulator = Emulator::new();
    create_grant(&emulator, GRANT).await;
    let cluster = "range-cluster";
    available_cluster(&emulator, cluster).await;
    start_copying(&emulator, cluster).await;

    // One row per bound that is overstepped: below and above the automated
    // range — whose values are day counts only, so the manual "indefinitely"
    // sentinel is outside it — and below, at and above the manual one.
    for (case, period, manual) in [
        ("automated period below the range", "0", None),
        ("automated period above the range", "36", None),
        ("automated period of the manual sentinel", "-1", None),
        ("automated period below the sentinel", "-2", None),
        ("automated period above the range, option false", "36", Some("false")),
        ("automated period of the largest integer", "2147483647", None),
        ("manual period below the sentinel", "-2", Some("true")),
        ("manual period of zero days", "0", Some("true")),
        ("manual period above the range", "3654", Some("true")),
    ] {
        let answer = modify(&emulator, cluster, period, manual).await;
        assert_refused(case, &answer, 400, "InvalidRetentionPeriodFault");
        let read = describe(&emulator, cluster).await;
        assert_eq!(
            copy_status(&read.body),
            Some(baseline()),
            "case {case}: expected the refusal to change nothing, got {:?}",
            copy_status(&read.body)
        );
    }

    // A period each range admits but the other does not is served on the side
    // that admits it, so the refusals above are the ranges and nothing else.
    for (case, period, manual) in [
        ("the longest automated period", "35", None),
        ("a manual period past the automated range", "3653", Some("true")),
    ] {
        let answer = modify(&emulator, cluster, period, manual).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the change served, got {} with body {}",
            answer.status, answer.body
        );
    }
}

/// The cluster a request names has to be one that can take the change: an absent
/// cluster, one still provisioning and one that is not copying at all are each
/// refused with their own modelled fault.
#[tokio::test]
async fn a_cluster_that_cannot_take_the_change_is_refused() {
    let emulator = Emulator::new();
    create_grant(&emulator, GRANT).await;

    // A cluster that is NOT there: the rows differ along the shape of the name
    // and along the period asked for, so the refusal follows the absence — and
    // precedes the range check, which the last row would otherwise fail.
    let longest: String = std::iter::once('c').chain(std::iter::repeat_n('x', 62)).collect();
    for (case, identifier, period) in [
        ("a name nothing holds", "no-such-cluster", "7"),
        ("a one-character name", "c", "1"),
        ("a name of the maximum length", longest.as_str(), "35"),
        ("a name nothing holds and a period out of range", "no-such-cluster", "0"),
    ] {
        let answer = modify(&emulator, identifier, period, None).await;
        assert_refused(case, &answer, 404, "ClusterNotFound");
    }

    // A cluster the service is still bringing up cannot take a configuration
    // change — and that refusal precedes the one for a cluster that is not
    // copying, which a cluster this young is as well. The clock is STOPPED
    // first, so the cluster stays inside its provisioning span however long the
    // calls take and the refusal is asserted outright.
    emulator.stop_clock();
    let created = emulator.create_minimal("provisioning-cluster").await;
    assert_eq!(
        created.status, 200,
        "case the provisioning cluster: expected it created, got {} with body {}",
        created.status, created.body
    );
    let during = modify(&emulator, "provisioning-cluster", "7", None).await;
    assert_refused("a cluster still creating", &during, 400, "InvalidClusterState");
    emulator.advance_clock(state::PROVISIONING_SPAN);

    // An available cluster that is not copying has no retention period to
    // change, whatever the request asks for.
    for (case, period, manual) in [
        ("an automated period on a cluster not copying", "7", None),
        ("a manual period on a cluster not copying", "-1", Some("true")),
    ] {
        let answer = modify(&emulator, "provisioning-cluster", period, manual).await;
        assert_refused(case, &answer, 400, "SnapshotCopyDisabledFault");
        let read = describe(&emulator, "provisioning-cluster").await;
        assert_eq!(
            copy_status(&read.body),
            None,
            "case {case}: expected the refusal to configure nothing, got {:?}",
            copy_status(&read.body)
        );
    }

    // Once the cluster IS copying the same request is served, so the refusals
    // above are the preconditions and nothing else.
    start_copying(&emulator, "provisioning-cluster").await;
    let after = modify(&emulator, "provisioning-cluster", "7", None).await;
    assert_eq!(
        after.status, 200,
        "case the copying cluster: expected the change served, got {} with body {}",
        after.status, after.body
    );
    assert_eq!(
        copy_status(&after.body),
        Some(expected("7", false)),
        "case the copying cluster: expected the new period reported, got {:?}",
        copy_status(&after.body)
    );
}
