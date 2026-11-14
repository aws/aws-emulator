// SPDX-License-Identifier: CC0-1.0

//! `RebootCluster`: restarting a provisioned cluster, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one recorded request: a restart of a
//! cluster in the available state reports it in the restarting state for the
//! momentary outage, applies the changes that were waiting for a restart, and
//! leaves the cluster available again once that span has passed, while a restart
//! of a cluster this account does not hold, or of one the service is already
//! working on, is refused and changes nothing. The tables span what the rule
//! generalises over: the cluster's shape (node type, node count, cluster type,
//! parameter group), the kind of change that was pending, and the states a
//! restart is refused in.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};
use crate::cluster_parameters::create_group;
use crate::modify_cluster_parameter_group::{create_cluster_on, membership_of, modify};

/// `call` takes borrowed pairs, so a case that builds owned parameters borrows
/// them through this.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// Cross the span a clock-owed change falls due after, by ADVANCING this
/// emulator's own clock across it rather than waiting for it: the transition is
/// then materialised by the next call whatever the host is doing, and the clock
/// stands still in between, so a transient state stays observable for as long as
/// a case needs it.
fn settle(emulator: &Emulator) {
    emulator.advance_clock(state::PROVISIONING_SPAN);
}

/// `RebootCluster` for one cluster identifier.
async fn reboot(emulator: &Emulator, cluster_identifier: &str) -> Answer {
    emulator
        .call(
            "RebootCluster",
            &[("ClusterIdentifier", cluster_identifier)],
        )
        .await
}

/// `DescribeClusters` for one cluster.
async fn describe(emulator: &Emulator, cluster_identifier: &str) -> Answer {
    emulator
        .call(
            "DescribeClusters",
            &[("ClusterIdentifier", cluster_identifier)],
        )
        .await
}

/// One row of the restart table: the case name, the cluster-shaping parameters
/// the create carries beyond the three required ones, and the node count the
/// cluster reports.
type ShapeCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
);

#[tokio::test]
async fn a_restart_reports_the_outage_and_ends_at_the_due_instant() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over — the node type, the
    // node count and the cluster type — because the restart is about the
    // cluster's STATE, not its configuration: every shape takes the same walk.
    let cases: Vec<ShapeCase> = vec![
        (
            "a single-node cluster",
            "reboot-single",
            &[("NodeType", "ra3.xlplus"), ("ClusterType", "single-node")],
            "1",
        ),
        (
            "a two-node cluster of another node type",
            "reboot-dc2-pair",
            &[
                ("NodeType", "dc2.large"),
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "2"),
            ],
            "2",
        ),
        (
            "a four-node cluster",
            "reboot-ra3-quad",
            &[
                ("NodeType", "ra3.4xlarge"),
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "4"),
            ],
            "4",
        ),
    ];

    for (case, identifier, shape, nodes) in cases {
        let mut params: Vec<(String, String)> = vec![
            ("ClusterIdentifier".to_string(), identifier.to_string()),
            ("MasterUsername".to_string(), "admin".to_string()),
        ];
        for (name, value) in shape {
            params.push(((*name).to_string(), (*value).to_string()));
        }
        let created = emulator.call("CreateCluster", &borrowed(&params)).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected the seed create to answer 200, got {} with body {}",
            created.status, created.body
        );
        // A restart is admitted in the available state, so the provisioning this
        // create started has to have fallen due first.
        settle(&emulator);

        let restarted = reboot(&emulator, identifier).await;
        assert_eq!(
            restarted.status, 200,
            "case {case}: expected 200, got {} with body {}",
            restarted.status, restarted.body
        );
        assert_eq!(
            restarted.text("ClusterIdentifier").as_deref(),
            Some(identifier),
            "case {case}: expected the restarted cluster back, got {:?}",
            restarted.text("ClusterIdentifier")
        );
        assert_eq!(
            restarted.text("ClusterStatus").as_deref(),
            Some(state::STATUS_REBOOTING),
            "case {case}: expected ClusterStatus={:?} during the outage, got {:?}",
            state::STATUS_REBOOTING,
            restarted.text("ClusterStatus")
        );
        assert_eq!(
            restarted.text("ClusterAvailabilityStatus").as_deref(),
            Some("Modifying"),
            "case {case}: expected ClusterAvailabilityStatus=Modifying during the outage, got {:?}",
            restarted.text("ClusterAvailabilityStatus")
        );
        // A restart changes no configuration: the cluster comes back with the
        // shape it had.
        assert_eq!(
            restarted.text("NumberOfNodes").as_deref(),
            Some(nodes),
            "case {case}: expected NumberOfNodes={nodes:?} to be untouched, got {:?}",
            restarted.text("NumberOfNodes")
        );
        assert_eq!(
            restarted.text("MasterUsername").as_deref(),
            Some("admin"),
            "case {case}: expected MasterUsername to be untouched, got {:?}",
            restarted.text("MasterUsername")
        );

        // The outage is observable to a later caller: the record a read reports
        // is the one the restart wrote.
        let during = describe(&emulator, identifier).await;
        assert_eq!(
            during.text("ClusterStatus").as_deref(),
            Some(state::STATUS_REBOOTING),
            "case {case}: expected a read during the outage to report {:?}, got {:?}",
            state::STATUS_REBOOTING,
            during.text("ClusterStatus")
        );

        // And it ENDS: the cluster is available again once the restart's span has
        // passed, whether or not anybody called in between.
        settle(&emulator);
        let after = describe(&emulator, identifier).await;
        assert_eq!(
            after.text("ClusterStatus").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {case}: expected the cluster to be {:?} again once the restart fell due, got {:?}",
            state::STATUS_AVAILABLE,
            after.text("ClusterStatus")
        );
        assert_eq!(
            after.text("ClusterAvailabilityStatus").as_deref(),
            Some("Available"),
            "case {case}: expected ClusterAvailabilityStatus=Available afterwards, got {:?}",
            after.text("ClusterAvailabilityStatus")
        );
        assert_eq!(
            after.text("NumberOfNodes").as_deref(),
            Some(nodes),
            "case {case}: expected NumberOfNodes={nodes:?} after the restart, got {:?}",
            after.text("NumberOfNodes")
        );

        // The same cluster takes another restart once it is available again, so
        // the walk is repeatable rather than a one-shot transition.
        let again = reboot(&emulator, identifier).await;
        assert_eq!(
            (again.status, again.text("ClusterStatus")),
            (200, Some(state::STATUS_REBOOTING.to_string())),
            "case {case}: expected a second restart to be accepted, got {} / {:?}",
            again.status,
            again.text("ClusterStatus")
        );
        settle(&emulator);
    }
}

/// One row of the refusal table: the case name, the identifier the call names,
/// and the status and error code the call must answer.
type RefusalCase = (&'static str, String, u16, &'static str);

#[tokio::test]
async fn a_cluster_that_cannot_take_a_restart_is_refused_and_unchanged() {
    let emulator = Emulator::new();

    // One cluster the service is already restarting: created, left to become
    // available, then restarted.
    let restarting = emulator.create_minimal("reboot-restarting").await;
    assert_eq!(
        restarting.status, 200,
        "case seed the restarting cluster: expected 200, got {} with body {}",
        restarting.status, restarting.body
    );
    settle(&emulator);
    let accepted = reboot(&emulator, "reboot-restarting").await;
    assert_eq!(
        accepted.status, 200,
        "case seed the restarting cluster: expected its first restart to be accepted, got {} with body {}",
        accepted.status, accepted.body
    );
    // And one cluster still being provisioned. The clock has stood still since
    // the settle above, so neither transient state can fall due while the table
    // runs — whatever the host is doing.
    let created = emulator.create_minimal("reboot-provisioning").await;
    assert_eq!(
        created.status, 200,
        "case seed the provisioning cluster: expected 200, got {} with body {}",
        created.status, created.body
    );

    let cases: Vec<RefusalCase> = vec![
        (
            "a cluster still being provisioned",
            "reboot-provisioning".to_string(),
            400,
            "InvalidClusterState",
        ),
        (
            "a cluster already restarting",
            "reboot-restarting".to_string(),
            400,
            "InvalidClusterState",
        ),
        (
            "an identifier nothing created",
            "no-such-cluster".to_string(),
            404,
            "ClusterNotFound",
        ),
        (
            "another identifier nothing created",
            "reboot-provisioning-2".to_string(),
            404,
            "ClusterNotFound",
        ),
        (
            "the longest identifier the rule admits, naming nothing",
            std::iter::once('c')
                .chain(std::iter::repeat_n('x', state::CLUSTER_IDENTIFIER_RULE.max_len - 1))
                .collect(),
            404,
            "ClusterNotFound",
        ),
    ];

    for (case, identifier, status, code) in cases {
        let refused = reboot(&emulator, &identifier).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (status, Some(code.to_string())),
            "case {case}: expected {status} {code}, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
    }

    // A refused restart changed NOTHING: the provisioning cluster was never put
    // into the restarting state, so it is simply available once its own span has
    // passed.
    settle(&emulator);
    let read = describe(&emulator, "reboot-provisioning").await;
    assert_eq!(
        read.text("ClusterStatus").as_deref(),
        Some(state::STATUS_AVAILABLE),
        "case a cluster still being provisioned: expected the refused call to leave it to become {:?}, got {:?}",
        state::STATUS_AVAILABLE,
        read.text("ClusterStatus")
    );
}

/// The refusal's MESSAGE is this operation's own sentence, not the one a read of
/// the same missing identifier answers with.
///
/// The rule: each operation words its `ClusterNotFound` refusal the way the
/// service words it for that operation, interpolating the identifier verbatim
/// from the request — so `RebootCluster` answers `Cluster not found: <id>` while
/// `DescribeClusters` answers `Cluster <id> not found.` for the very same name.
/// The rows differ along what the rule generalises over: the identifier's
/// spelling, its length (both documented bounds), its case, and whether it never
/// existed or was created and then deleted.
#[tokio::test]
async fn the_not_found_message_is_this_operations_own_sentence() {
    let emulator = Emulator::new();

    // One cluster that DOES exist, so a refusal below cannot be an "everything
    // is refused" artefact, and one created-then-deleted name — the shape the
    // lifecycle takes: a restart after the delete names nothing.
    let present = emulator.create_minimal("reboot-message-present").await;
    assert_eq!(
        present.status, 200,
        "case seed the present cluster: expected 200, got {} with body {}",
        present.status, present.body
    );
    let doomed = emulator.create_minimal("reboot-message-deleted").await;
    assert_eq!(
        doomed.status, 200,
        "case seed the doomed cluster: expected 200, got {} with body {}",
        doomed.status, doomed.body
    );
    let deleted = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "reboot-message-deleted"),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the doomed cluster: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );

    // `max_len` is the documented bound on `ClusterIdentifier`; see
    // `state::CLUSTER_IDENTIFIER_RULE`.
    let longest: String = std::iter::once('c')
        .chain(std::iter::repeat_n('x', state::CLUSTER_IDENTIFIER_RULE.max_len - 1))
        .collect();
    let cases: Vec<(&str, String)> = vec![
        ("a plain identifier nothing created", "no-reboot-target".to_string()),
        (
            "the shortest identifier the documented rule admits",
            "c".to_string(),
        ),
        ("the longest identifier the documented rule admits", longest),
        (
            "an identifier one character off a stored one",
            "reboot-message-presen".to_string(),
        ),
        (
            "an identifier in mixed case",
            "Reboot-Message-Absent".to_string(),
        ),
        (
            "a cluster this account created and then deleted",
            "reboot-message-deleted".to_string(),
        ),
    ];

    for (case, identifier) in cases {
        let refused = reboot(&emulator, &identifier).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (404, Some("ClusterNotFound".to_string())),
            "case {case}: expected 404 ClusterNotFound, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        let expected = format!("Cluster not found: {identifier}");
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(expected.as_str()),
            "case {case}: expected the message {expected:?}, got {:?} with body {}",
            refused.text("Message"),
            refused.body
        );

        // The SAME missing identifier, read rather than restarted, carries the
        // read's own sentence: the two templates are distinguishable.
        let read = describe(&emulator, &identifier).await;
        let expected_read = format!("Cluster {identifier} not found.");
        assert_eq!(
            read.text("Message").as_deref(),
            Some(expected_read.as_str()),
            "case {case}: expected the read's message {expected_read:?}, got {:?} with body {}",
            read.text("Message"),
            read.body
        );
        assert_ne!(
            refused.text("Message"),
            read.text("Message"),
            "case {case}: expected the two operations to word the refusal differently, got \
             {:?} from both",
            refused.text("Message")
        );
    }

    // The stored cluster is still restartable once it is available: the rows
    // above refused the NAME, not the operation.
    settle(&emulator);
    let accepted = reboot(&emulator, "reboot-message-present").await;
    assert_eq!(
        accepted.status, 200,
        "case the stored cluster still restarts: expected 200, got {} with body {}",
        accepted.status, accepted.body
    );
}

/// One row of the pending-parameter table: the case name, the group and cluster
/// the row uses, the name-value pairs set on the group, and the apply status
/// every membership entry reports BEFORE the restart.
type PendingCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
);

#[tokio::test]
async fn a_restart_applies_the_parameter_changes_that_were_waiting_for_one() {
    let emulator = Emulator::new();

    // The rows differ along the kind and the number of changes waiting: one
    // static parameter, two static parameters, and a dynamic-only change that
    // was never waiting for a restart at all.
    let cases: Vec<PendingCase> = vec![
        (
            "one static parameter",
            "reboot-static-one",
            "reboot-cluster-one",
            &[("require_ssl", "true")],
            state::PARAMETER_APPLY_STATUS_PENDING_REBOOT,
        ),
        (
            "two static parameters",
            "reboot-static-two",
            "reboot-cluster-two",
            &[
                ("require_ssl", "true"),
                ("enable_case_sensitive_identifier", "true"),
            ],
            state::PARAMETER_APPLY_STATUS_PENDING_REBOOT,
        ),
        (
            "a dynamic-only change, which never waited for a restart",
            "reboot-dynamic",
            "reboot-cluster-three",
            &[("query_group", "report")],
            state::PARAMETER_APPLY_STATUS_IN_SYNC,
        ),
    ];

    for (case, group, identifier, parameters, before) in cases {
        create_group(&emulator, group).await;
        create_cluster_on(&emulator, identifier, group).await;
        settle(&emulator);

        let changed = modify(&emulator, group, parameters).await;
        assert_eq!(
            changed.status, 200,
            "case {case}: expected the parameter change to answer 200, got {} with body {}",
            changed.status, changed.body
        );
        let (statuses, _) = membership_of(&emulator, identifier).await;
        assert!(
            !statuses.is_empty()
                && statuses
                    .iter()
                    .all(|status| status == before),
            "case {case}: expected every apply status to be {before:?} before the restart, got {statuses:?}"
        );

        let restarted = reboot(&emulator, identifier).await;
        assert_eq!(
            restarted.status, 200,
            "case {case}: expected the restart to answer 200, got {} with body {}",
            restarted.status, restarted.body
        );
        // The restart is what applies a value the cluster had not seen, so the
        // call itself reports the membership in sync.
        let reported = all_text(&restarted.body, "ParameterApplyStatus");
        assert!(
            !reported.is_empty()
                && reported
                    .iter()
                    .all(|status| status == state::PARAMETER_APPLY_STATUS_IN_SYNC),
            "case {case}: expected the restart to report every apply status as {:?}, got {reported:?}",
            state::PARAMETER_APPLY_STATUS_IN_SYNC
        );
        // And a later read agrees: nothing is waiting for another restart.
        let (statuses, _) = membership_of(&emulator, identifier).await;
        assert!(
            !statuses.is_empty()
                && statuses
                    .iter()
                    .all(|status| status == state::PARAMETER_APPLY_STATUS_IN_SYNC),
            "case {case}: expected every apply status to be {:?} after the restart, got {statuses:?}",
            state::PARAMETER_APPLY_STATUS_IN_SYNC
        );
        settle(&emulator);
    }
}

#[tokio::test]
async fn a_restart_applies_the_pending_cluster_modification() {
    let emulator = Emulator::new();
    let identifier = "reboot-pending-password";
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case seed the cluster: expected 200, got {} with body {}",
        created.status, created.body
    );
    settle(&emulator);

    // A new admin password is applied asynchronously, so the modify leaves it in
    // `PendingModifiedValues` — reported as the mask, never the password.
    let modified = emulator
        .call(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("MasterUserPassword", "An0therPassw0rd!"),
            ],
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "case modify the admin password: expected 200, got {} with body {}",
        modified.status, modified.body
    );
    assert_eq!(
        modified.text("MasterUserPassword").as_deref(),
        Some(state::REDACTED_MASTER_USER_PASSWORD),
        "case modify the admin password: expected the pending change to be reported as the mask, got {:?}",
        modified.text("MasterUserPassword")
    );
    assert!(
        !modified.body.contains("An0therPassw0rd"),
        "case modify the admin password: the password must never be echoed, got body {}",
        modified.body
    );

    // The restart applies the pending modification, so from then on nothing is
    // pending on the cluster.
    let restarted = reboot(&emulator, identifier).await;
    assert_eq!(
        restarted.status, 200,
        "case restart with a pending modification: expected 200, got {} with body {}",
        restarted.status, restarted.body
    );
    assert_eq!(
        restarted.text("MasterUserPassword"),
        None,
        "case restart with a pending modification: expected nothing pending after the restart, got {:?}",
        restarted.text("MasterUserPassword")
    );
    let read = describe(&emulator, identifier).await;
    assert_eq!(
        read.text("MasterUserPassword"),
        None,
        "case restart with a pending modification: expected a later read to report nothing pending, got {:?}",
        read.text("MasterUserPassword")
    );
}
