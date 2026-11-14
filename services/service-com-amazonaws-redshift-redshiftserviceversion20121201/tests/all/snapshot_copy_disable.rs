// SPDX-License-Identifier: CC0-1.0

//! Disabling cross-Region snapshot copy on a cluster, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness and the
//! `ClusterSnapshotCopyStatus` reader are shared with
//! [`crate::cluster_lifecycle`] and [`crate::snapshot_copy_configuration`]), so
//! each assertion covers the whole walk: form-urlencoded deserialize -> handler
//! -> XML serialize.
//!
//! The RULE the suite asserts: `DisableSnapshotCopy` withdraws the WHOLE
//! cross-Region copy configuration the named cluster carries — destination,
//! both retention periods and the grant together — so the answer and every later
//! read of that cluster report no `ClusterSnapshotCopyStatus` at all, the grant
//! the copies were encrypted under stops counting as in use, and the cluster
//! itself is otherwise untouched; and it withdraws nothing when a precondition
//! the model states fails: an absent cluster, one not in the available state, or
//! one that is not copying at all.
//!
//! The tables span what the rule generalises over — which members the withdrawn
//! configuration carried, the destination Region, each kind of retention period
//! including the published bounds, whether a grant was named, and how many
//! enable/disable cycles the cluster has been through — rather than one recorded
//! request.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};
use crate::snapshot_copy_configuration::{
    assert_refused, available_cluster, available_clusters, copy_status, create_grant, describe,
    enable, GRANT,
};

/// One row of the withdrawal table: the configuration the cluster is put on
/// before the disable, which is what the disable has to withdraw.
struct DisableCase {
    case: &'static str,
    destination: &'static str,
    /// The optional members the enable carries, as request parameters.
    extra: &'static [(&'static str, &'static str)],
}

/// `DisableSnapshotCopy` for one cluster.
async fn disable(emulator: &Emulator, cluster: &str) -> Answer {
    emulator
        .call("DisableSnapshotCopy", &[("ClusterIdentifier", cluster)])
        .await
}

/// Whatever configuration a cluster carried, the disable leaves it reporting
/// none, and leaves the cluster the available cluster it was.
#[tokio::test]
async fn the_whole_configuration_is_withdrawn_whatever_it_carried() {
    let emulator = Emulator::new();
    create_grant(&emulator, GRANT).await;

    // The rows differ along which optional members the withdrawn configuration
    // carried, the area and number of its destination Region, each published
    // bound of each retention range, and whether the copies were encrypted under
    // a grant.
    let rows = [
        DisableCase {
            case: "a configuration of the destination alone",
            destination: "us-east-1",
            extra: &[],
        },
        DisableCase {
            case: "the shortest automated retention",
            destination: "eu-west-3",
            extra: &[("RetentionPeriod", "1")],
        },
        DisableCase {
            case: "the longest automated retention",
            destination: "ap-southeast-4",
            extra: &[("RetentionPeriod", "35")],
        },
        DisableCase {
            case: "manual snapshots retained indefinitely",
            destination: "ca-central-1",
            extra: &[
                ("RetentionPeriod", "14"),
                ("ManualSnapshotRetentionPeriod", "-1"),
            ],
        },
        DisableCase {
            case: "the longest manual retention",
            destination: "me-central-1",
            extra: &[
                ("RetentionPeriod", "20"),
                ("ManualSnapshotRetentionPeriod", "3653"),
            ],
        },
        DisableCase {
            case: "a partition-qualified destination",
            destination: "us-gov-west-1",
            extra: &[("ManualSnapshotRetentionPeriod", "1")],
        },
        DisableCase {
            case: "copies encrypted under a grant",
            destination: "us-west-1",
            extra: &[
                ("RetentionPeriod", "7"),
                ("ManualSnapshotRetentionPeriod", "30"),
                ("SnapshotCopyGrantName", GRANT),
            ],
        },
    ];

    let clusters: Vec<String> = (0..rows.len())
        .map(|index| format!("disable-cluster-{index}"))
        .collect();
    available_clusters(&emulator, &clusters).await;

    for (row, cluster) in rows.iter().zip(&clusters) {
        let case = row.case;

        let enabled = enable(&emulator, cluster, row.destination, row.extra).await;
        assert_eq!(
            enabled.status, 200,
            "case {case}: expected the copy configured first, got {} with body {}",
            enabled.status, enabled.body
        );
        // The cluster really is carrying a configuration, so the absence the
        // assertions below read is one the disable produced.
        assert!(
            copy_status(&enabled.body).is_some(),
            "case {case}: expected a configuration to withdraw, got {:?}",
            copy_status(&enabled.body)
        );
        let node_count_before = describe(&emulator, cluster).await.text("NumberOfNodes");

        let answer = disable(&emulator, cluster).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the disable served, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            copy_status(&answer.body),
            None,
            "case {case}: expected the answer to report no copy status, got {:?}",
            copy_status(&answer.body)
        );
        // Withdrawing a standing instruction is not work done TO the cluster, so
        // the cluster is still the available cluster it was, and the members it
        // derives from its own configuration stand.
        assert_eq!(
            answer.text("ClusterStatus").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {case}: expected the cluster to stay {}, got {:?}",
            state::STATUS_AVAILABLE,
            answer.text("ClusterStatus")
        );
        assert_eq!(
            answer.text("ClusterIdentifier").as_deref(),
            Some(cluster.as_str()),
            "case {case}: expected the cluster reported back, got {:?}",
            answer.text("ClusterIdentifier")
        );

        let read = describe(&emulator, cluster).await;
        assert_eq!(
            copy_status(&read.body),
            None,
            "case {case}: expected a later read to report no copy status, got {:?}",
            copy_status(&read.body)
        );
        assert_eq!(
            read.text("NumberOfNodes"),
            node_count_before,
            "case {case}: expected the cluster's own members untouched, got {:?}",
            read.text("NumberOfNodes")
        );
    }

    // No cluster copies under the grant any more, so the grant is no longer in
    // use and the delete path — which reads the same one record — serves it.
    let deleted = emulator
        .call("DeleteSnapshotCopyGrant", &[("SnapshotCopyGrantName", GRANT)])
        .await;
    assert_eq!(
        deleted.status, 200,
        "case the grant no cluster copies under: expected the delete served, got {} with body {}",
        deleted.status, deleted.body
    );
}

/// Enable and disable are inverses, so a cluster can be put back on a copy
/// configuration after a disable, as many times and with as many different
/// configurations as asked.
#[tokio::test]
async fn a_disabled_cluster_can_be_enabled_again() {
    let emulator = Emulator::new();
    create_grant(&emulator, GRANT).await;
    let cluster = "cycling-cluster";
    available_cluster(&emulator, cluster).await;

    // Each cycle configures something DIFFERENT, so a reported value cannot be
    // one an earlier cycle left behind.
    for (case, destination, extra) in [
        ("the first cycle", "us-east-2", &[][..]),
        (
            "a second cycle with a grant",
            "eu-north-1",
            &[("RetentionPeriod", "3"), ("SnapshotCopyGrantName", GRANT)][..],
        ),
        (
            "a third cycle with both periods",
            "ap-south-1",
            &[
                ("RetentionPeriod", "35"),
                ("ManualSnapshotRetentionPeriod", "3653"),
            ][..],
        ),
    ] {
        let enabled = enable(&emulator, cluster, destination, extra).await;
        assert_eq!(
            enabled.status, 200,
            "case {case}: expected the enable served, got {} with body {}",
            enabled.status, enabled.body
        );
        assert_eq!(
            copy_status(&enabled.body).and_then(|status| status.destination_region),
            Some(destination.to_string()),
            "case {case}: expected the new destination reported, got {:?}",
            copy_status(&enabled.body)
        );

        let answer = disable(&emulator, cluster).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the disable served, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            copy_status(&answer.body),
            None,
            "case {case}: expected the configuration withdrawn, got {:?}",
            copy_status(&answer.body)
        );
        let read = describe(&emulator, cluster).await;
        assert_eq!(
            copy_status(&read.body),
            None,
            "case {case}: expected a later read to report none, got {:?}",
            copy_status(&read.body)
        );
    }
}

/// The cluster a request names has to be one that can take the withdrawal: an
/// absent cluster, one still provisioning and one that is not copying at all are
/// each refused with their own modelled fault, and the refusal withdraws nothing.
#[tokio::test]
async fn a_cluster_that_cannot_take_the_withdrawal_is_refused() {
    let emulator = Emulator::new();

    // A cluster that is NOT there: the rows differ along the shape of the name,
    // so the refusal follows the absence and not the spelling.
    let longest: String = std::iter::once('c')
        .chain(std::iter::repeat_n('x', 62))
        .collect();
    for (case, identifier) in [
        ("a name nothing holds", "no-such-cluster"),
        ("a one-character name", "c"),
        ("a name of the maximum length", longest.as_str()),
    ] {
        let answer = disable(&emulator, identifier).await;
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
    let during = disable(&emulator, "provisioning-cluster").await;
    assert_refused(
        "a cluster still creating",
        &during,
        400,
        "InvalidClusterState",
    );
    emulator.advance_clock(state::PROVISIONING_SPAN);

    // An available cluster that is not copying has nothing to withdraw: one that
    // never was configured, and one whose configuration an earlier disable
    // already withdrew.
    let answer = disable(&emulator, "provisioning-cluster").await;
    assert_refused(
        "a cluster never configured",
        &answer,
        400,
        "SnapshotCopyAlreadyDisabledFault",
    );

    let enabled = enable(&emulator, "provisioning-cluster", "us-east-1", &[]).await;
    assert_eq!(
        enabled.status, 200,
        "case the enable before the disable: expected it served, got {} with body {}",
        enabled.status, enabled.body
    );
    let first = disable(&emulator, "provisioning-cluster").await;
    assert_eq!(
        first.status, 200,
        "case the first disable: expected it served, got {} with body {}",
        first.status, first.body
    );
    for case in ["the second disable", "the third disable"] {
        let again = disable(&emulator, "provisioning-cluster").await;
        assert_refused(case, &again, 400, "SnapshotCopyAlreadyDisabledFault");
        let read = describe(&emulator, "provisioning-cluster").await;
        assert_eq!(
            copy_status(&read.body),
            None,
            "case {case}: expected the cluster to stay on no configuration, got {:?}",
            copy_status(&read.body)
        );
    }

    // A refusal on a cluster that IS copying leaves that configuration standing:
    // the absent-cluster rows above are refused for the absence, so a cluster
    // beside them keeps what it carries.
    create_grant(&emulator, GRANT).await;
    available_cluster(&emulator, "bystander-cluster").await;
    let enabled = enable(
        &emulator,
        "bystander-cluster",
        "eu-west-1",
        &[("SnapshotCopyGrantName", GRANT)],
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "case the bystander enable: expected it served, got {} with body {}",
        enabled.status, enabled.body
    );
    let refused = disable(&emulator, "no-such-cluster").await;
    assert_refused("a bystander's neighbour", &refused, 404, "ClusterNotFound");
    let read = describe(&emulator, "bystander-cluster").await;
    assert_eq!(
        copy_status(&read.body).and_then(|status| status.snapshot_copy_grant_name),
        Some(GRANT.to_string()),
        "case a bystander's neighbour: expected the bystander's configuration to stand, got {:?}",
        copy_status(&read.body)
    );
}
