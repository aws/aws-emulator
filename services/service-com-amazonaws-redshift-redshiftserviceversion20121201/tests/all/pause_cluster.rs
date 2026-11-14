// SPDX-License-Identifier: CC0-1.0

//! `PauseCluster`: suspending the compute of a provisioned cluster, end to end
//! over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one recorded request: a pause is legal
//! only from a cluster that is serving, it leaves the cluster suspended once the
//! change has fallen due, and a cluster in any other state — still being
//! provisioned, already being worked on, already suspended — is refused with the
//! state fault and left exactly as it stands. The tables span what the rule
//! generalises over: the cluster's shape, the state the pause is attempted from,
//! and the identifier's spelling and length.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// `PauseCluster` for one cluster identifier.
async fn pause(emulator: &Emulator, cluster_identifier: &str) -> Answer {
    emulator
        .call("PauseCluster", &[("ClusterIdentifier", cluster_identifier)])
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

/// Wait for `identifier` to reach `status` by POLLING the read, so the wait ends
/// on the CONDITION rather than after a fixed time.
async fn poll_for_status(emulator: &Emulator, case: &str, identifier: &str, status: &str) {
    let (read, _) = emulator
        .poll_until(
            case,
            "DescribeClusters",
            &[("ClusterIdentifier", identifier)],
            |answer| answer.text("ClusterStatus").as_deref() == Some(status),
        )
        .await;
    assert_eq!(
        read.text("ClusterStatus").as_deref(),
        Some(status),
        "case {case}: expected the cluster to reach {status:?}, got {:?}",
        read.text("ClusterStatus")
    );
}

/// One row of the pause table: the case name, the identifier, and the
/// cluster-shaping parameters the create carries beyond the required ones.
type ShapeCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

#[tokio::test]
async fn a_pause_of_a_running_cluster_suspends_it_at_the_due_instant() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over — the node type, the
    // node count and the cluster type — because the pause is about the cluster's
    // STATE, not its configuration: every shape takes the same walk.
    let cases: Vec<ShapeCase> = vec![
        (
            "a single-node cluster",
            "pause-single",
            &[("NodeType", "ra3.xlplus"), ("ClusterType", "single-node")],
        ),
        (
            "a two-node cluster of another node type",
            "pause-dc2-pair",
            &[
                ("NodeType", "dc2.large"),
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "2"),
            ],
        ),
        (
            "a four-node cluster",
            "pause-ra3-quad",
            &[
                ("NodeType", "ra3.4xlarge"),
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "4"),
            ],
        ),
    ];

    for (case, identifier, shape) in cases {
        let mut params: Vec<(&str, &str)> = vec![
            ("ClusterIdentifier", identifier),
            ("MasterUsername", "admin"),
        ];
        params.extend_from_slice(shape);
        let created = emulator.call("CreateCluster", &params).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected the seed create to answer 200, got {} with body {}",
            created.status, created.body
        );
        // A pause is admitted only from the serving state, so the provisioning
        // this create started has to have fallen due first.
        poll_for_status(&emulator, case, identifier, state::STATUS_AVAILABLE).await;

        let paused = pause(&emulator, identifier).await;
        assert_eq!(
            paused.status, 200,
            "case {case}: expected 200, got {} with body {}",
            paused.status, paused.body
        );
        assert_eq!(
            paused.text("ClusterIdentifier").as_deref(),
            Some(identifier),
            "case {case}: expected the paused cluster back, got {:?}",
            paused.text("ClusterIdentifier")
        );
        // The span is never zero, so the caller of the pause sees the change
        // being applied rather than a cluster that is already suspended.
        assert_eq!(
            paused.text("ClusterStatus").as_deref(),
            Some(state::STATUS_MODIFYING),
            "case {case}: expected ClusterStatus={:?} while the pause is applied, got {:?}",
            state::STATUS_MODIFYING,
            paused.text("ClusterStatus")
        );
        assert_eq!(
            paused.text("ClusterAvailabilityStatus").as_deref(),
            Some("Modifying"),
            "case {case}: expected ClusterAvailabilityStatus=Modifying, got {:?}",
            paused.text("ClusterAvailabilityStatus")
        );

        // And it ENDS suspended — reached by polling the READ alone, so only the
        // clock can have moved it.
        poll_for_status(&emulator, case, identifier, state::STATUS_PAUSED).await;
        let suspended = describe(&emulator, identifier).await;
        assert_eq!(
            suspended.text("ClusterAvailabilityStatus").as_deref(),
            Some("Unavailable"),
            "case {case}: expected a suspended cluster to be unavailable for queries, got {:?}",
            suspended.text("ClusterAvailabilityStatus")
        );
        assert_eq!(
            suspended.text("MasterUsername").as_deref(),
            Some("admin"),
            "case {case}: expected the cluster's configuration to be untouched, got {:?}",
            suspended.text("MasterUsername")
        );

        // A suspended cluster takes no second pause: the state precondition is
        // the same rule, and `paused` is not the state a pause is legal from.
        let again = pause(&emulator, identifier).await;
        assert_eq!(
            (again.status, again.error_code().as_deref()),
            (400, Some("InvalidClusterState")),
            "case {case}: expected a pause of a suspended cluster to be refused, got {} / {:?} \
             with body {}",
            again.status,
            again.error_code(),
            again.body
        );
        assert_eq!(
            again.text("Message").as_deref(),
            Some(state::PAUSE_REQUIRES_ACTIVE_CLUSTER),
            "case {case}: expected Amazon Redshift's own sentence, got {:?}",
            again.text("Message")
        );
    }
}

#[tokio::test]
async fn a_pause_of_a_cluster_still_being_provisioned_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();

    // The clock is STOPPED before the create, so the cluster stays inside its
    // provisioning span however long the calls below take: what is asserted is
    // the refusal a cluster the service is still bringing up carries, and no
    // part of the case depends on how fast the host runs.
    emulator.stop_clock();

    let identifier = "pause-creating";
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.text("ClusterStatus").as_deref(),
        Some(state::STATUS_CREATING),
        "case create {identifier}: the span is never zero, so the caller sees the transient \
         status, got {:?}",
        created.text("ClusterStatus")
    );
    let refused = pause(&emulator, identifier).await;

    assert_eq!(
        (refused.status, refused.error_code().as_deref()),
        (400, Some("InvalidClusterState")),
        "case a cluster still being provisioned: expected 400 InvalidClusterState, got {} / {:?} \
         with body {}",
        refused.status,
        refused.error_code(),
        refused.body
    );
    // Fixed text with no variable part: the refusal names the state a pause is
    // legal from, not the cluster and not the state it is in.
    assert_eq!(
        refused.text("Message").as_deref(),
        Some(state::PAUSE_REQUIRES_ACTIVE_CLUSTER),
        "case a cluster still being provisioned: expected Amazon Redshift's own sentence, got {:?}",
        refused.text("Message")
    );
    assert!(
        !refused.text("Message").unwrap_or_default().contains(identifier),
        "case a cluster still being provisioned: the sentence has no variable part, so it must \
         not name the cluster, got {:?}",
        refused.text("Message")
    );

    // The refused pause changed NOTHING: the cluster simply becomes available
    // once its own provisioning span has passed, rather than suspended — and the
    // span is crossed by advancing the stopped clock, not by waiting for it.
    emulator.advance_clock(state::PROVISIONING_SPAN);
    poll_for_status(
        &emulator,
        "a cluster still being provisioned",
        identifier,
        state::STATUS_AVAILABLE,
    )
    .await;
}

#[tokio::test]
async fn a_pause_of_a_cluster_this_account_does_not_hold_is_refused_as_not_found() {
    let emulator = Emulator::new();

    // One cluster that DOES exist, so a refusal below cannot be an "everything
    // is refused" artefact, and one created-then-deleted name — the shape the
    // lifecycle takes: a pause after the delete names nothing.
    let present = emulator.create_minimal("pause-message-present").await;
    assert_eq!(
        present.status, 200,
        "case seed the present cluster: expected 200, got {} with body {}",
        present.status, present.body
    );
    let doomed = emulator.create_minimal("pause-message-deleted").await;
    assert_eq!(
        doomed.status, 200,
        "case seed the doomed cluster: expected 200, got {} with body {}",
        doomed.status, doomed.body
    );
    let deleted = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "pause-message-deleted"),
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
        .chain(std::iter::repeat_n(
            'x',
            state::CLUSTER_IDENTIFIER_RULE.max_len - 1,
        ))
        .collect();
    let cases: Vec<(&str, String)> = vec![
        (
            "a plain identifier nothing created",
            "no-pause-target".to_string(),
        ),
        (
            "the shortest identifier the documented rule admits",
            "c".to_string(),
        ),
        ("the longest identifier the documented rule admits", longest),
        (
            "an identifier one character off a stored one",
            "pause-message-presen".to_string(),
        ),
        (
            "an identifier in mixed case",
            "Pause-Message-Absent".to_string(),
        ),
        (
            "a cluster this account created and then deleted",
            "pause-message-deleted".to_string(),
        ),
    ];

    for (case, identifier) in cases {
        let refused = pause(&emulator, &identifier).await;
        assert_eq!(
            (refused.status, refused.error_code().as_deref()),
            (404, Some("ClusterNotFound")),
            "case {case}: expected 404 ClusterNotFound, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        // This operation carries the sentence every cluster-scoped operation
        // without a recorded one of its own carries, with the identifier taken
        // verbatim from the request.
        let expected = format!("Cluster {identifier} not found.");
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(expected.as_str()),
            "case {case}: expected the message {expected:?}, got {:?} with body {}",
            refused.text("Message"),
            refused.body
        );
    }

    // The stored cluster is still pausable once it is serving: the rows above
    // refused the NAME, not the operation.
    poll_for_status(
        &emulator,
        "the stored cluster still pauses",
        "pause-message-present",
        state::STATUS_AVAILABLE,
    )
    .await;
    let accepted = pause(&emulator, "pause-message-present").await;
    assert_eq!(
        accepted.status, 200,
        "case the stored cluster still pauses: expected 200, got {} with body {}",
        accepted.status, accepted.body
    );
}
