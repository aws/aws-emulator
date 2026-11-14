// SPDX-License-Identifier: CC0-1.0

//! What the operations that change a cluster's COMPUTE do: resuming suspended
//! compute, rotating the cluster's encryption keys, resizing it and giving that
//! resize up — end to end over the awsQuery wire.
//!
//! The RULES these cases assert, each over a table rather than over one recorded
//! request:
//!
//! * a resume is legal only from a cluster whose compute is SUSPENDED, so a
//!   cluster in any other state is refused with `InvalidClusterStateFault` and
//!   keeps the state it was in — the mirror of the rule `PauseCluster` enforces;
//! * rotating a cluster's encryption keys needs a cluster that HAS encryption
//!   enabled, which is that operation's own documented constraint on the
//!   identifier it is given;
//! * a resize applies the shape the request asks for, records the resize a later
//!   `DescribeResize` reports, and leaves the cluster serving again at the due
//!   instant;
//! * a resize can be given up only while it is still running, and the cancel puts
//!   the cluster back on the shape the resize found it on.
//!
//! Every wait is a poll for the CONDITION through the shared harness
//! ([`Emulator::poll_until`]), never a sleep, so a loaded host costs a case more
//! polls rather than failing it. The harness is shared with
//! [`crate::cluster_lifecycle`], which owns it.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// The parameters one row of a table sends beside the cluster it names.
type Params = Vec<(&'static str, &'static str)>;

/// One row of the resize table: the case, the resize's parameters, the node count
/// the cluster ends on and the `ResizeType` the record reports.
type ResizeRow = (&'static str, Params, &'static str, &'static str);

/// One row of the elastic-restriction table: the case, the parameters and whether
/// the resize is served.
type RestrictionRow = (&'static str, Params, bool);

/// Create the cluster `identifier` with `extra` beside the three required
/// members, and assert it was created.
async fn create(emulator: &Emulator, case: &str, identifier: &str, extra: &[(&str, &str)]) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", identifier),
        ("NodeType", "ra3.xlplus"),
        ("MasterUsername", "admin"),
    ];
    params.extend_from_slice(extra);
    let created = emulator.call("CreateCluster", &params).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateCluster {identifier} expected 200, got {} with body {}",
        created.status, created.body
    );
    created
}

/// Poll `DescribeClusters` until the cluster reports `status`.
async fn await_status(emulator: &Emulator, case: &str, identifier: &str, status: &str) -> Answer {
    let (answer, _) = emulator
        .poll_until(
            case,
            "DescribeClusters",
            &[("ClusterIdentifier", identifier)],
            |answer| answer.text("ClusterStatus").as_deref() == Some(status),
        )
        .await;
    answer
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

/// A paused cluster resumes: the call reports the change being applied and the
/// cluster is serving again at the due instant, which a later read reports.
#[tokio::test]
async fn a_paused_cluster_resumes_and_serves_again() {
    let emulator = Emulator::new();
    let identifier = "resumable-cluster";
    create(&emulator, "a paused cluster resumes", identifier, &[]).await;
    await_status(&emulator, "the cluster is serving", identifier, state::STATUS_AVAILABLE).await;

    let paused = emulator
        .call("PauseCluster", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        paused.status, 200,
        "case pause the cluster: expected 200, got {} with body {}",
        paused.status, paused.body
    );
    await_status(&emulator, "the cluster is paused", identifier, state::STATUS_PAUSED).await;

    let resumed = emulator
        .call("ResumeCluster", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        resumed.status, 200,
        "case resume the cluster: expected 200, got {} with body {}",
        resumed.status, resumed.body
    );
    // The resume reports the state the service applies a change in — the cluster
    // is not serving yet, so it reports neither `paused` nor `available`.
    assert_eq!(
        resumed.text("ClusterStatus").as_deref(),
        Some(state::STATUS_MODIFYING),
        "case resume the cluster: expected ClusterStatus={}, got {:?}",
        state::STATUS_MODIFYING,
        resumed.text("ClusterStatus")
    );
    // … and it is serving at the due instant, which the clock owes whether or not
    // anybody calls.
    let served = await_status(
        &emulator,
        "the resumed cluster serves again",
        identifier,
        state::STATUS_AVAILABLE,
    )
    .await;
    assert_eq!(
        served.text("ClusterAvailabilityStatus").as_deref(),
        Some(state::AVAILABILITY_STATUS_AVAILABLE),
        "case the resumed cluster serves again: expected ClusterAvailabilityStatus={}, got {:?}",
        state::AVAILABILITY_STATUS_AVAILABLE,
        served.text("ClusterAvailabilityStatus")
    );
}

/// A resume of a cluster whose compute is NOT suspended is refused, and the
/// cluster keeps the state it was in — for a table spanning the states a cluster
/// can be in when the call arrives.
#[tokio::test]
async fn resuming_a_cluster_that_is_not_paused_is_refused() {
    let emulator = Emulator::new();

    // (case, the state the cluster is in when the resume arrives, how to get it
    // there)
    let rows: Vec<(&str, &str)> = vec![
        ("a cluster that is still being provisioned", state::STATUS_CREATING),
        ("a cluster that is serving", state::STATUS_AVAILABLE),
    ];

    for (index, (case, state_wanted)) in rows.into_iter().enumerate() {
        let identifier = format!("unpaused-cluster-{index}");
        create(&emulator, case, &identifier, &[]).await;
        if state_wanted == state::STATUS_AVAILABLE {
            await_status(&emulator, case, &identifier, state::STATUS_AVAILABLE).await;
        }

        let refused = emulator
            .call("ResumeCluster", &[("ClusterIdentifier", &identifier)])
            .await;
        // A cluster still being provisioned may have settled before the call
        // arrived, in which case this row is the "serving" row: either way the
        // resume is refused with the same fault.
        assert_refused(case, &refused, 400, "InvalidClusterState");
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(state::RESUME_REQUIRES_PAUSED_CLUSTER),
            "case {case}: expected the state a resume is legal from named, got {:?}",
            refused.text("Message")
        );
    }

    // A second resume of a cluster the first one already resumed is refused the
    // same way: the cluster is no longer suspended.
    let identifier = "double-resume-cluster";
    create(&emulator, "a second resume", identifier, &[]).await;
    await_status(&emulator, "a second resume", identifier, state::STATUS_AVAILABLE).await;
    let paused = emulator
        .call("PauseCluster", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        paused.status, 200,
        "case a second resume: expected the pause served, got {} with body {}",
        paused.status, paused.body
    );
    await_status(&emulator, "a second resume", identifier, state::STATUS_PAUSED).await;
    let first = emulator
        .call("ResumeCluster", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        first.status, 200,
        "case a second resume: expected the first resume served, got {} with body {}",
        first.status, first.body
    );
    let second = emulator
        .call("ResumeCluster", &[("ClusterIdentifier", identifier)])
        .await;
    assert_refused("a second resume", &second, 400, "InvalidClusterState");
}

/// Rotating a cluster's encryption keys needs a cluster that has encryption
/// enabled: an encrypted cluster takes the rotation and reports it, and an
/// unencrypted one is refused on the value it named.
#[tokio::test]
async fn rotating_encryption_keys_needs_an_encrypted_cluster() {
    let emulator = Emulator::new();

    // (case, the `Encrypted` the create asked for, whether the rotation is taken)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("an encrypted cluster", "true", true),
        ("an unencrypted cluster", "false", false),
        // A create that names nothing gets this service's default, which is an
        // ENCRYPTED cluster (`CreateCluster` stores `Encrypted` as true unless the
        // request turns it off), so the silent-request case takes the rotation.
        ("a cluster whose create named no encryption", "", true),
    ];

    for (index, (case, encrypted, rotates)) in rows.into_iter().enumerate() {
        let identifier = format!("rotating-cluster-{index}");
        let extra: Vec<(&str, &str)> = if encrypted.is_empty() {
            Vec::new()
        } else {
            vec![("Encrypted", encrypted)]
        };
        create(&emulator, case, &identifier, &extra).await;
        await_status(&emulator, case, &identifier, state::STATUS_AVAILABLE).await;

        let rotated = emulator
            .call("RotateEncryptionKey", &[("ClusterIdentifier", &identifier)])
            .await;
        if rotates {
            assert_eq!(
                rotated.status, 200,
                "case {case}: expected the rotation served, got {} with body {}",
                rotated.status, rotated.body
            );
            // The published `ClusterStatus` vocabulary has its own value for the
            // work, and the cluster is serving again at the due instant.
            assert_eq!(
                rotated.text("ClusterStatus").as_deref(),
                Some(state::STATUS_ROTATING_KEYS),
                "case {case}: expected ClusterStatus={}, got {:?}",
                state::STATUS_ROTATING_KEYS,
                rotated.text("ClusterStatus")
            );
            await_status(&emulator, case, &identifier, state::STATUS_AVAILABLE).await;
        } else {
            assert_refused(case, &rotated, 400, "InvalidParameterValue");
            // The cluster is untouched: the refusal happened before any change.
            let described = emulator
                .call("DescribeClusters", &[("ClusterIdentifier", &identifier)])
                .await;
            assert_eq!(
                described.text("ClusterStatus").as_deref(),
                Some(state::STATUS_AVAILABLE),
                "case {case}: expected the cluster still serving, got {:?}",
                described.text("ClusterStatus")
            );
        }
    }
}

/// A resize applies the shape the request asks for, records the resize a later
/// `DescribeResize` reports, and leaves the cluster serving again — for a table
/// spanning the node count, the cluster type and the resize kind.
#[tokio::test]
async fn a_resize_applies_the_shape_and_records_the_resize() {
    let emulator = Emulator::new();

    // (case, the resize's parameters, the node count the cluster ends on, the
    // `ResizeType` the record reports)
    let rows: Vec<ResizeRow> = vec![
        (
            "more nodes of the same type",
            vec![("NumberOfNodes", "4")],
            "4",
            state::RESIZE_TYPE_ELASTIC,
        ),
        (
            "the smallest multi-node cluster",
            vec![("NumberOfNodes", "2")],
            "2",
            state::RESIZE_TYPE_ELASTIC,
        ),
        (
            "a classic resize onto another node type",
            vec![
                ("Classic", "true"),
                ("NodeType", "ra3.4xlarge"),
                ("NumberOfNodes", "3"),
            ],
            "3",
            state::RESIZE_TYPE_CLASSIC,
        ),
        (
            "a classic resize back to one node",
            vec![("Classic", "true"), ("ClusterType", "single-node")],
            "1",
            state::RESIZE_TYPE_CLASSIC,
        ),
    ];

    for (index, (case, parameters, nodes, resize_type)) in rows.into_iter().enumerate() {
        let identifier = format!("resized-cluster-{index}");
        create(&emulator, case, &identifier, &[("NumberOfNodes", "2")]).await;
        await_status(&emulator, case, &identifier, state::STATUS_AVAILABLE).await;

        let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", &identifier)];
        params.extend(parameters.iter().copied());
        let resized = emulator.call("ResizeCluster", &params).await;
        assert_eq!(
            resized.status, 200,
            "case {case}: expected the resize served, got {} with body {}",
            resized.status, resized.body
        );
        assert_eq!(
            resized.text("NumberOfNodes").as_deref(),
            Some(nodes),
            "case {case}: expected NumberOfNodes={nodes}, got {:?}",
            resized.text("NumberOfNodes")
        );

        // The resize is readable as a resize, with the shape it is moving to and
        // the kind it is.
        let progress = emulator
            .call("DescribeResize", &[("ClusterIdentifier", &identifier)])
            .await;
        assert_eq!(
            progress.status, 200,
            "case {case}: expected the resize readable, got {} with body {}",
            progress.status, progress.body
        );
        assert_eq!(
            progress.text("ResizeType").as_deref(),
            Some(resize_type),
            "case {case}: expected ResizeType={resize_type}, got {:?}",
            progress.text("ResizeType")
        );
        assert_eq!(
            progress.text("TargetNumberOfNodes").as_deref(),
            Some(nodes),
            "case {case}: expected TargetNumberOfNodes={nodes}, got {:?}",
            progress.text("TargetNumberOfNodes")
        );

        // The cluster is serving again at the due instant, on the shape the
        // resize asked for, and the resize record reports it finished.
        let served = await_status(&emulator, case, &identifier, state::STATUS_AVAILABLE).await;
        assert_eq!(
            served.text("NumberOfNodes").as_deref(),
            Some(nodes),
            "case {case}: expected the stored cluster on {nodes} nodes, got {:?}",
            served.text("NumberOfNodes")
        );
        let (finished, _) = emulator
            .poll_until(
                case,
                "DescribeResize",
                &[("ClusterIdentifier", &identifier)],
                |answer| {
                    answer.text("Status").as_deref() == Some(state::RESIZE_STATUS_SUCCEEDED)
                },
            )
            .await;
        assert_eq!(
            finished.text("Status").as_deref(),
            Some(state::RESIZE_STATUS_SUCCEEDED),
            "case {case}: expected the resize to report {}, got {:?}",
            state::RESIZE_STATUS_SUCCEEDED,
            finished.text("Status")
        );
    }
}

/// An elastic resize keeps the cluster's node type: a request that asks to change
/// it is refused with the option fault, while the same request as a CLASSIC resize
/// is served.
#[tokio::test]
async fn an_elastic_resize_cannot_change_the_node_type() {
    let emulator = Emulator::new();
    let identifier = "elastic-resize-cluster";
    create(
        &emulator,
        "an elastic resize keeps the node type",
        identifier,
        &[("NumberOfNodes", "2")],
    )
    .await;
    await_status(
        &emulator,
        "an elastic resize keeps the node type",
        identifier,
        state::STATUS_AVAILABLE,
    )
    .await;

    // (case, the parameters, whether the resize is served)
    let rows: Vec<RestrictionRow> = vec![
        (
            "an elastic resize onto another node type",
            vec![("NodeType", "ra3.4xlarge")],
            false,
        ),
        (
            "an elastic resize restating the cluster's own node type",
            vec![("NodeType", "ra3.xlplus"), ("NumberOfNodes", "3")],
            true,
        ),
        (
            "a classic resize onto another node type",
            vec![
                ("Classic", "true"),
                ("NodeType", "ra3.4xlarge"),
                ("NumberOfNodes", "3"),
            ],
            true,
        ),
    ];

    for (case, parameters, served) in rows {
        await_status(&emulator, case, identifier, state::STATUS_AVAILABLE).await;
        let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", identifier)];
        params.extend(parameters.iter().copied());
        let answer = emulator.call("ResizeCluster", &params).await;
        if served {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the resize served, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "UnsupportedOptionFault");
        }
    }
}

/// A resize can be given up only while it is still RUNNING: a cluster that has
/// never been resized, and one whose last resize has finished, answer the modelled
/// `ResizeNotFoundFault`; a resize in flight is cancelled and the cluster goes
/// back onto the shape it had.
#[tokio::test]
async fn only_a_running_resize_can_be_cancelled() {
    let emulator = Emulator::new();

    // A cluster nothing has ever resized has no resize to cancel.
    let never = "never-resized-cluster";
    create(&emulator, "a cluster never resized", never, &[]).await;
    await_status(
        &emulator,
        "a cluster never resized",
        never,
        state::STATUS_AVAILABLE,
    )
    .await;
    let refused = emulator
        .call("CancelResize", &[("ClusterIdentifier", never)])
        .await;
    assert_refused("a cluster never resized", &refused, 404, "ResizeNotFound");

    // A resize that has already finished is no longer running, so it cannot be
    // given up either.
    let finished = "finished-resize-cluster";
    create(&emulator, "a finished resize", finished, &[("NumberOfNodes", "2")]).await;
    await_status(&emulator, "a finished resize", finished, state::STATUS_AVAILABLE).await;
    let resized = emulator
        .call(
            "ResizeCluster",
            &[("ClusterIdentifier", finished), ("NumberOfNodes", "4")],
        )
        .await;
    assert_eq!(
        resized.status, 200,
        "case a finished resize: expected the resize served, got {} with body {}",
        resized.status, resized.body
    );
    emulator
        .poll_until(
            "a finished resize",
            "DescribeResize",
            &[("ClusterIdentifier", finished)],
            |answer| answer.text("Status").as_deref() == Some(state::RESIZE_STATUS_SUCCEEDED),
        )
        .await;
    let too_late = emulator
        .call("CancelResize", &[("ClusterIdentifier", finished)])
        .await;
    assert_refused("a finished resize", &too_late, 404, "ResizeNotFound");

    // A resize still in flight IS cancelled, and the cluster goes back onto the
    // shape the resize found it on. The clock is STOPPED once the cluster is
    // serving, so the resize below is still running when the cancel reaches it
    // however long the host takes over the two calls.
    let identifier = "cancelled-resize-cluster";
    create(
        &emulator,
        "a running resize is cancelled",
        identifier,
        &[("NumberOfNodes", "2")],
    )
    .await;
    await_status(
        &emulator,
        "a running resize is cancelled",
        identifier,
        state::STATUS_AVAILABLE,
    )
    .await;
    emulator.stop_clock();
    let resized = emulator
        .call(
            "ResizeCluster",
            &[("ClusterIdentifier", identifier), ("NumberOfNodes", "6")],
        )
        .await;
    assert_eq!(
        resized.status, 200,
        "case a running resize is cancelled: expected the resize served, got {} with body {}",
        resized.status, resized.body
    );
    let answer = emulator
        .call("CancelResize", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case a running resize is cancelled: expected the cancel of a resize still running to \
         be served, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.text("Status").as_deref(),
        Some(state::RESIZE_STATUS_CANCELLING),
        "case a running resize is cancelled: expected Status={}, got {:?} with body {}",
        state::RESIZE_STATUS_CANCELLING,
        answer.text("Status"),
        answer.body
    );
    // The cancel reports the shape the cluster is going BACK to, which is the one
    // it had before the resize.
    assert_eq!(
        answer.text("TargetNumberOfNodes").as_deref(),
        Some("2"),
        "case a running resize is cancelled: expected the original node count as the target, \
         got {:?}",
        answer.text("TargetNumberOfNodes")
    );
    // … and the cluster really is back on it once the cancellation falls due —
    // reached by advancing the stopped clock across the span the cancellation is
    // owed, not by waiting for it.
    emulator.advance_clock(state::PROVISIONING_SPAN);
    let served = await_status(
        &emulator,
        "a running resize is cancelled",
        identifier,
        state::STATUS_AVAILABLE,
    )
    .await;
    assert_eq!(
        served.text("NumberOfNodes").as_deref(),
        Some("2"),
        "case a running resize is cancelled: expected the cluster back on 2 nodes, got {:?}",
        served.text("NumberOfNodes")
    );
    // The resize record never reports success: it did not complete.
    let (progress, _) = emulator
        .poll_until(
            "a cancelled resize never succeeds",
            "DescribeResize",
            &[("ClusterIdentifier", identifier)],
            |answer| answer.text("Status").as_deref() == Some(state::RESIZE_STATUS_FAILED),
        )
        .await;
    assert_eq!(
        progress.text("Status").as_deref(),
        Some(state::RESIZE_STATUS_FAILED),
        "case a cancelled resize never succeeds: expected Status={}, got {:?}",
        state::RESIZE_STATUS_FAILED,
        progress.text("Status")
    );
}
