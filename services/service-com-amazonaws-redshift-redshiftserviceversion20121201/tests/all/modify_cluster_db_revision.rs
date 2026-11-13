// SPDX-License-Identifier: CC0-1.0

//! `ModifyClusterDbRevision` and `ModifyAquaConfiguration`: the two cluster
//! modifications whose answer is decided before any revision or AQUA work, end to
//! end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! Two rules are under test, and each row is one member of the class the rule
//! generalises over rather than one recorded request:
//!
//! 1. `ModifyClusterDbRevision` resolves the CLUSTER first — before the revision
//!    the request names is looked at at all — so a cluster this account does not
//!    hold is `ClusterNotFound` (404), a cluster the service is still working on
//!    is `InvalidClusterState` (400), and a cluster that runs the latest revision
//!    this service publishes a target of is `ClusterOnLatestRevision` (400).
//! 2. `ModifyAquaConfiguration` is RETIRED: every call is answered with Amazon
//!    Redshift's internal-failure refusal, whatever the request names and whether
//!    or not the cluster exists, and no cluster changes.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// Create one cluster and wait for it to be serving, by POLLING the read.
async fn available_cluster(emulator: &Emulator, identifier: &str) {
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {identifier}: expected 200, got {} with body {}",
        created.status, created.body
    );
    emulator
        .poll_until(
            &format!("seed cluster {identifier}"),
            "DescribeClusters",
            &[("ClusterIdentifier", identifier)],
            |answer| answer.text("ClusterStatus").as_deref() == Some(state::STATUS_AVAILABLE),
        )
        .await;
}

/// `ModifyClusterDbRevision` for one cluster and revision target.
async fn modify_revision(emulator: &Emulator, cluster: &str, target: &str) -> Answer {
    emulator
        .call(
            "ModifyClusterDbRevision",
            &[
                ("ClusterIdentifier", cluster),
                ("RevisionTarget", target),
            ],
        )
        .await
}

#[tokio::test]
async fn a_revision_change_resolves_the_cluster_before_the_revision() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "revision-present").await;

    // The class: identifiers this account holds no cluster under, crossed with
    // the revision target asked for — the target cannot change the answer,
    // because the cluster is resolved first.
    let longest: String = std::iter::once('c')
        .chain(std::iter::repeat_n(
            'x',
            state::CLUSTER_IDENTIFIER_RULE.max_len - 1,
        ))
        .collect();
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a plain identifier nothing created",
            "no-revision-target".to_string(),
            "bogus-revision",
        ),
        (
            "the shortest identifier the documented rule admits",
            "c".to_string(),
            "bogus-revision",
        ),
        (
            "the longest identifier the documented rule admits",
            longest,
            "bogus-revision",
        ),
        (
            "an identifier one character off a stored one",
            "revision-presen".to_string(),
            // A revision target of the shape this service's own revision
            // numbers take, so the refusal is about the cluster and not the
            // target's spelling.
            "1.0.12345",
        ),
        (
            "an identifier in mixed case",
            "Revision-Present".to_string(),
            "",
        ),
    ];

    for (case, identifier, target) in rows {
        let refused = modify_revision(&emulator, &identifier, target).await;
        assert_eq!(
            (refused.status, refused.error_code().as_deref()),
            (404, Some("ClusterNotFound")),
            "case {case:?}: expected 404 ClusterNotFound, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        // The identifier the request named is interpolated verbatim into the
        // sentence this operation carries.
        let expected = format!("Cluster {identifier} not found.");
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {:?}",
            refused.text("Message")
        );
    }
}

/// One row of the latest-revision table: the case name, the cluster identifier,
/// and the cluster-shaping parameters the create carries beyond the required
/// ones.
type ShapeCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

#[tokio::test]
async fn a_cluster_on_its_latest_revision_has_nothing_to_be_moved_onto() {
    let emulator = Emulator::new();

    // The rows differ along the cluster's shape, because the rule is about the
    // revisions PUBLISHED for a cluster rather than about its configuration.
    let shapes: Vec<ShapeCase> = vec![
        (
            "a single-node cluster",
            "revision-single",
            &[("NodeType", "ra3.xlplus"), ("ClusterType", "single-node")],
        ),
        (
            "a two-node cluster of another node type",
            "revision-dc2-pair",
            &[
                ("NodeType", "dc2.large"),
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "2"),
            ],
        ),
    ];

    for (case, identifier, shape) in shapes {
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
        emulator
            .poll_until(
                case,
                "DescribeClusters",
                &[("ClusterIdentifier", identifier)],
                |answer| answer.text("ClusterStatus").as_deref() == Some(state::STATUS_AVAILABLE),
            )
            .await;

        // The revisions a cluster can be moved onto are the ones its own
        // `RevisionTargets` publishes, and a read of them reports none.
        let published = emulator
            .call(
                "DescribeClusterDbRevisions",
                &[("ClusterIdentifier", identifier)],
            )
            .await;
        assert!(
            all_text(&published.body, "DatabaseRevision").is_empty(),
            "case {case}: expected the cluster to publish no later revision, got body {}",
            published.body
        );

        // So every revision target — the one the read reports as current
        // included — leaves nothing to move the cluster onto.
        let current = published
            .text("CurrentDatabaseRevision")
            .expect("the read reports the revision the cluster runs");
        for target in [current.as_str(), "bogus-revision", ""] {
            let refused = modify_revision(&emulator, identifier, target).await;
            assert_eq!(
                (refused.status, refused.error_code().as_deref()),
                (400, Some("ClusterOnLatestRevision")),
                "case {case} with target {target:?}: expected 400 ClusterOnLatestRevision, got \
                 {} / {:?} with body {}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                refused.text("Message").as_deref(),
                Some(
                    format!("Cluster {identifier} is already on the latest database revision.")
                        .as_str()
                ),
                "case {case} with target {target:?}: expected the cluster named in the sentence, \
                 got {:?}",
                refused.text("Message")
            );
        }

        // And the refused calls changed nothing: the cluster still runs the
        // revision it ran, and is still serving.
        let after = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
            .await;
        assert_eq!(
            (
                after.text("ClusterRevisionNumber"),
                after.text("ClusterStatus")
            ),
            (Some(current), Some(state::STATUS_AVAILABLE.to_string())),
            "case {case}: expected the cluster untouched, got body {}",
            after.body
        );
    }
}

#[tokio::test]
async fn the_retired_aqua_path_answers_the_internal_failure_refusal_for_every_call() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "aqua-cluster").await;

    // The class: every call, whatever it names. The cluster is NOT resolved, so
    // a request naming one this account holds is answered exactly as one naming
    // a cluster that was never created.
    let rows: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "a cluster this account does not hold",
            vec![
                ("ClusterIdentifier", "no-such-cluster"),
                ("AquaConfigurationStatus", "enabled"),
            ],
        ),
        (
            "a cluster this account holds, asking for AQUA",
            vec![
                ("ClusterIdentifier", "aqua-cluster"),
                ("AquaConfigurationStatus", "enabled"),
            ],
        ),
        (
            "a cluster this account holds, asking for no AQUA",
            vec![
                ("ClusterIdentifier", "aqua-cluster"),
                ("AquaConfigurationStatus", "disabled"),
            ],
        ),
        (
            "the automatic configuration",
            vec![
                ("ClusterIdentifier", "aqua-cluster"),
                ("AquaConfigurationStatus", "auto"),
            ],
        ),
        (
            "no configuration status at all",
            vec![("ClusterIdentifier", "aqua-cluster")],
        ),
    ];

    for (case, params) in rows {
        let refused = emulator.call("ModifyAquaConfiguration", &params).await;
        assert_eq!(
            (refused.status, refused.error_code().as_deref()),
            (500, Some("InternalFailure")),
            "case {case:?}: expected 500 InternalFailure, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        // Fixed text end to end: the refusal is about the retired path, so
        // nothing in it comes from the request.
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(state::AQUA_CONFIGURATION_INTERNAL_FAILURE),
            "case {case:?}: expected Amazon Redshift's own sentence, got {:?}",
            refused.text("Message")
        );
        assert_eq!(
            refused.text("AquaConfiguration"),
            None,
            "case {case:?}: expected no configuration in a refusal, got {:?}",
            refused.text("AquaConfiguration")
        );
    }

    // The cluster is untouched, and the AQUA configuration it reports is the
    // automatic one every cluster carries — which the retired calls neither
    // changed nor were able to read.
    let read = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", "aqua-cluster")])
        .await;
    assert_eq!(
        (read.text("AquaStatus"), read.text("AquaConfigurationStatus")),
        (Some("disabled".to_string()), Some("auto".to_string())),
        "case the cluster after the retired calls: expected the automatic configuration, got \
         body {}",
        read.body
    );
}
