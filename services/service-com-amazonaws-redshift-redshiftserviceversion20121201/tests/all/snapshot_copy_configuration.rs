// SPDX-License-Identifier: CC0-1.0

//! Enabling cross-region snapshot copy on a cluster, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The RULE the suite asserts: `EnableSnapshotCopy` records the copy
//! configuration ON the cluster — the destination Region, the automated and the
//! manual retention periods in effect (the documented default for a period the
//! request omitted) and the snapshot copy grant the copies are encrypted
//! under — so the answer and every later read of that cluster report the same
//! `ClusterSnapshotCopyStatus`, and the grant counts as in use by the cluster;
//! and it records nothing when a precondition the model states fails: an absent
//! cluster, one not in the available state, one already copying, a destination
//! that is not a Region code, a retention period outside its published range, or
//! a grant this account does not hold.
//!
//! The tables span what the rule generalises over — which members the request
//! carries, the Region's area, qualifier and number, each published bound of
//! each retention range and one value past it — rather than one recorded
//! request.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// One row of the enable table: the optional members the request carries beside
/// the destination Region, and what the cluster must then report.
struct EnableCase {
    case: &'static str,
    destination: &'static str,
    /// `RetentionPeriod`, or `None` when the request names none.
    retention: Option<&'static str>,
    /// `ManualSnapshotRetentionPeriod`, or `None` when the request names none.
    manual_retention: Option<&'static str>,
    /// `SnapshotCopyGrantName`, or `None` when the request names none.
    grant: Option<&'static str>,
}

/// The four members of a cluster's `ClusterSnapshotCopyStatus`, as a response
/// carries them.
///
/// `pub(crate)` so a sibling suite about the SAME record reads it through this
/// one reader rather than writing a second copy of it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CopyStatus {
    pub(crate) destination_region: Option<String>,
    pub(crate) retention_period: Option<String>,
    pub(crate) manual_snapshot_retention_period: Option<String>,
    pub(crate) snapshot_copy_grant_name: Option<String>,
}

/// The grant every row that names one is configured with.
pub(crate) const GRANT: &str = "copies-under-this-grant";

/// The `<ClusterSnapshotCopyStatus>` a body carries, or `None` when it carries
/// none.
///
/// The element is sliced out before its members are read because a `Cluster`
/// carries a `ManualSnapshotRetentionPeriod` of its own: only the children of
/// this element are the copy configuration.
pub(crate) fn copy_status(body: &str) -> Option<CopyStatus> {
    let open = "<ClusterSnapshotCopyStatus>";
    let close = "</ClusterSnapshotCopyStatus>";
    let start = body.find(open)? + open.len();
    let end = body[start..].find(close)? + start;
    let inner = &body[start..end];
    let member = |tag: &str| all_text(inner, tag).into_iter().next();
    Some(CopyStatus {
        destination_region: member("DestinationRegion"),
        retention_period: member("RetentionPeriod"),
        manual_snapshot_retention_period: member("ManualSnapshotRetentionPeriod"),
        snapshot_copy_grant_name: member("SnapshotCopyGrantName"),
    })
}

/// `EnableSnapshotCopy` with the members one row carries.
pub(crate) async fn enable(
    emulator: &Emulator,
    cluster: &str,
    destination: &str,
    extra: &[(&str, &str)],
) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", cluster),
        ("DestinationRegion", destination),
    ];
    params.extend_from_slice(extra);
    emulator.call("EnableSnapshotCopy", &params).await
}

/// The optional members of one row, as request parameters.
fn optional_params(row: &EnableCase) -> Vec<(&'static str, &'static str)> {
    let mut params = Vec::new();
    if let Some(retention) = row.retention {
        params.push(("RetentionPeriod", retention));
    }
    if let Some(manual) = row.manual_retention {
        params.push(("ManualSnapshotRetentionPeriod", manual));
    }
    if let Some(grant) = row.grant {
        params.push(("SnapshotCopyGrantName", grant));
    }
    params
}

/// `DescribeClusters` for one cluster.
pub(crate) async fn describe(emulator: &Emulator, cluster: &str) -> Answer {
    emulator
        .call("DescribeClusters", &[("ClusterIdentifier", cluster)])
        .await
}

/// Create one cluster and wait out the clock-owed `creating` -> `available`
/// transition, which is the state a configuration change needs it in.
pub(crate) async fn available_cluster(emulator: &Emulator, identifier: &str) {
    available_clusters(emulator, std::slice::from_ref(&identifier.to_string())).await;
}

/// Create every cluster a table's rows need, then cross the provisioning span
/// ONCE by ADVANCING this emulator's own clock: the span is owed from each
/// create, and the last one started it.
///
/// Advancing rather than waiting also stops the clock, so a record a later case
/// means to read in its transient state stays in it however long the calls take.
pub(crate) async fn available_clusters(emulator: &Emulator, identifiers: &[String]) {
    for identifier in identifiers {
        let created = emulator.create_minimal(identifier).await;
        assert_eq!(
            created.status, 200,
            "case {identifier}: the cluster must be created, got {} with body {}",
            created.status, created.body
        );
    }
    emulator.advance_clock(state::PROVISIONING_SPAN);
}

/// Create the grant the rows that name one are configured with.
pub(crate) async fn create_grant(emulator: &Emulator, name: &str) {
    let created = emulator
        .call("CreateSnapshotCopyGrant", &[("SnapshotCopyGrantName", name)])
        .await;
    assert_eq!(
        created.status, 200,
        "case the grant {name}: the create must be served, got {} with body {}",
        created.status, created.body
    );
}

/// Assert one refusal row: the answer's status and `<Code>`.
pub(crate) fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
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

/// The configuration a request asks for is what the answer reports and what
/// every later read of the cluster reports, with the documented default standing
/// in for each period the request omitted.
#[tokio::test]
async fn an_enabled_copy_is_reported_exactly_as_the_request_configured_it() {
    let emulator = Emulator::new();
    create_grant(&emulator, GRANT).await;

    // The rows differ along which optional members the request carries, the
    // area, the partition qualifier and the number of the destination Region,
    // and each published bound of each retention range.
    let rows = [
        EnableCase {
            case: "no optional member",
            destination: "us-east-1",
            retention: None,
            manual_retention: None,
            grant: None,
        },
        EnableCase {
            case: "the shortest automated retention",
            destination: "eu-west-3",
            retention: Some("1"),
            manual_retention: None,
            grant: None,
        },
        EnableCase {
            case: "the longest automated retention",
            destination: "ap-southeast-4",
            retention: Some("35"),
            manual_retention: None,
            grant: None,
        },
        EnableCase {
            case: "manual snapshots retained indefinitely",
            destination: "ca-central-1",
            retention: Some("14"),
            manual_retention: Some("-1"),
            grant: None,
        },
        EnableCase {
            case: "the shortest manual retention",
            destination: "sa-east-1",
            retention: None,
            manual_retention: Some("1"),
            grant: None,
        },
        EnableCase {
            case: "the longest manual retention",
            destination: "me-central-1",
            retention: Some("20"),
            manual_retention: Some("3653"),
            grant: None,
        },
        EnableCase {
            case: "a partition-qualified destination",
            destination: "us-gov-west-1",
            retention: None,
            manual_retention: None,
            grant: None,
        },
        EnableCase {
            case: "copies encrypted under a grant",
            destination: "us-west-1",
            retention: Some("7"),
            manual_retention: Some("30"),
            grant: Some(GRANT),
        },
    ];

    let clusters: Vec<String> = (0..rows.len())
        .map(|index| format!("copy-cluster-{index}"))
        .collect();
    available_clusters(&emulator, &clusters).await;

    for (row, cluster) in rows.iter().zip(&clusters) {
        let case = row.case;

        // Before the enable the cluster is on no copy configuration at all, so
        // the members below cannot be anything the create left behind.
        let before = describe(&emulator, cluster).await;
        assert_eq!(
            copy_status(&before.body),
            None,
            "case {case}: expected a fresh cluster to report no copy status, got {:?}",
            copy_status(&before.body)
        );

        let expected = CopyStatus {
            destination_region: Some(row.destination.to_string()),
            retention_period: Some(row.retention.map_or_else(
                || state::DEFAULT_SNAPSHOT_COPY_RETENTION.to_string(),
                str::to_string,
            )),
            manual_snapshot_retention_period: Some(row.manual_retention.map_or_else(
                || state::MANUAL_RETENTION_INDEFINITE.to_string(),
                str::to_string,
            )),
            snapshot_copy_grant_name: row.grant.map(str::to_string),
        };

        let answer = enable(&emulator, cluster, row.destination, &optional_params(row)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the enable served, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            copy_status(&answer.body).as_ref(),
            Some(&expected),
            "case {case}: expected the answer to report {expected:?}, got {:?}",
            copy_status(&answer.body)
        );
        // Configuring the copy is not work done TO the cluster, so the cluster
        // is still the available cluster it was.
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
            Some(&expected),
            "case {case}: expected a later read to report {expected:?}, got {:?}",
            copy_status(&read.body)
        );
    }

    // The grant a cluster copies under is in use by that cluster, which is what
    // the delete path reads: one record, written by the enable and read by
    // another operation.
    let refused = emulator
        .call("DeleteSnapshotCopyGrant", &[("SnapshotCopyGrantName", GRANT)])
        .await;
    assert_refused(
        "the grant a cluster copies under",
        &refused,
        400,
        "InvalidSnapshotCopyGrantStateFault",
    );
}

/// A destination is accepted exactly when it is an Amazon Web Services Region
/// code, and a value that is not one configures nothing.
#[tokio::test]
async fn a_destination_is_accepted_exactly_when_it_names_a_region() {
    let emulator = Emulator::new();

    // Region codes from the published list, spanning its areas, its location
    // words, a partition qualifier and a Region number past the first.
    let accepted = [
        "us-east-1",
        "eu-west-2",
        "ap-northeast-3",
        "af-south-1",
        "il-central-1",
        "mx-central-1",
        "cn-north-1",
        "us-gov-east-1",
        "us-isob-east-1",
    ];
    let clusters: Vec<String> = (0..accepted.len())
        .map(|index| format!("region-cluster-{index}"))
        .collect();
    available_clusters(&emulator, &clusters).await;
    for (destination, cluster) in accepted.iter().zip(&clusters) {
        let answer = enable(&emulator, cluster, destination, &[]).await;
        assert_eq!(
            answer.status, 200,
            "case {destination}: expected the enable served, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            copy_status(&answer.body).and_then(|status| status.destination_region),
            Some((*destination).to_string()),
            "case {destination}: expected the destination reported back, got {:?}",
            copy_status(&answer.body)
        );
    }

    // Each refused value fails one part of the form: the number, the location
    // word, the area code, the case, the qualifier count, or the segment count.
    let refused = [
        "",
        "us-east",
        "useast1",
        "US-EAST-1",
        "us-east-0",
        "us-east-01",
        "us-east-1a",
        "us-east-one",
        "us-middle-1",
        "zz-east-1",
        "us-gov-iso-east-1",
        "us--east-1",
    ];
    let cluster = "region-refusal-cluster";
    available_cluster(&emulator, cluster).await;
    for destination in refused {
        let answer = enable(&emulator, cluster, destination, &[]).await;
        assert_refused(destination, &answer, 404, "UnknownSnapshotCopyRegionFault");
        // The refusal configured nothing, so the same cluster can take the next
        // row: a cluster already copying would be refused for THAT instead.
        let read = describe(&emulator, cluster).await;
        assert_eq!(
            copy_status(&read.body),
            None,
            "case {destination:?}: expected the refusal to configure nothing, got {:?}",
            copy_status(&read.body)
        );
    }
}

/// Each retention period is accepted exactly inside its published range, and a
/// value outside it configures nothing.
#[tokio::test]
async fn a_retention_period_is_accepted_exactly_inside_its_published_range() {
    let emulator = Emulator::new();
    let cluster = "retention-cluster";
    available_cluster(&emulator, cluster).await;

    // One row per bound that is overstepped: below and above the automated
    // range, and below, at and above the manual one — whose only value under 1
    // is the "retained indefinitely" sentinel.
    for (case, member, value) in [
        ("automated retention below the range", "RetentionPeriod", "0"),
        ("automated retention above the range", "RetentionPeriod", "36"),
        (
            "automated retention of the manual sentinel",
            "RetentionPeriod",
            "-1",
        ),
        (
            "manual retention below the sentinel",
            "ManualSnapshotRetentionPeriod",
            "-2",
        ),
        (
            "manual retention of zero days",
            "ManualSnapshotRetentionPeriod",
            "0",
        ),
        (
            "manual retention above the range",
            "ManualSnapshotRetentionPeriod",
            "3654",
        ),
    ] {
        let answer = enable(&emulator, cluster, "us-east-1", &[(member, value)]).await;
        assert_refused(case, &answer, 400, "InvalidRetentionPeriodFault");
        let read = describe(&emulator, cluster).await;
        assert_eq!(
            copy_status(&read.body),
            None,
            "case {case}: expected the refusal to configure nothing, got {:?}",
            copy_status(&read.body)
        );
    }

    // The same request inside both ranges is served, so the refusals above are
    // the periods and nothing else.
    let answer = enable(
        &emulator,
        cluster,
        "us-east-1",
        &[("RetentionPeriod", "35"), ("ManualSnapshotRetentionPeriod", "-1")],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case both periods inside their ranges: expected the enable served, got {} with body {}",
        answer.status, answer.body
    );
}

/// The cluster a request names has to be one that can take the configuration: an
/// absent cluster, one still provisioning and one already copying are each
/// refused with their own modelled fault, and a grant this account does not hold
/// is refused as well.
#[tokio::test]
async fn a_cluster_that_cannot_take_the_configuration_is_refused() {
    let emulator = Emulator::new();

    // A cluster that is NOT there: the rows differ along the shape of the name,
    // so the refusal follows the absence and not the spelling.
    let longest: String = std::iter::once('c').chain(std::iter::repeat_n('x', 62)).collect();
    for (case, identifier) in [
        ("a name nothing holds", "no-such-cluster"),
        ("a one-character name", "c"),
        ("a name of the maximum length", longest.as_str()),
    ] {
        let answer = enable(&emulator, identifier, "us-east-1", &[]).await;
        assert_refused(case, &answer, 404, "ClusterNotFound");
    }

    // A cluster the service is still bringing up cannot take a configuration
    // change. The clock is STOPPED first, so the cluster stays inside its
    // provisioning span however long the calls take and the refusal is asserted
    // outright.
    emulator.stop_clock();
    let created = emulator.create_minimal("provisioning-cluster").await;
    assert_eq!(
        created.status, 200,
        "case the provisioning cluster: expected it created, got {} with body {}",
        created.status, created.body
    );
    let during = enable(&emulator, "provisioning-cluster", "us-east-1", &[]).await;
    assert_refused("a cluster still creating", &during, 400, "InvalidClusterState");
    emulator.advance_clock(state::PROVISIONING_SPAN);

    // Past the due instant the same request is served, and the refusal before it
    // configured nothing.
    let after = enable(&emulator, "provisioning-cluster", "us-east-1", &[]).await;
    assert_eq!(
        after.status, 200,
        "case the settled cluster: expected the enable served, got {} with body {}",
        after.status, after.body
    );

    // A cluster that is already copying is refused, whatever the second request
    // asks for, and keeps the configuration the first one left.
    for (case, destination, extra) in [
        ("the same destination", "us-east-1", Vec::new()),
        ("another destination", "eu-west-1", Vec::new()),
        (
            "another retention period",
            "us-east-1",
            vec![("RetentionPeriod", "21")],
        ),
    ] {
        let answer = enable(&emulator, "provisioning-cluster", destination, &extra).await;
        assert_refused(case, &answer, 400, "SnapshotCopyAlreadyEnabledFault");
        let read = describe(&emulator, "provisioning-cluster").await;
        assert_eq!(
            copy_status(&read.body),
            Some(CopyStatus {
                destination_region: Some("us-east-1".to_string()),
                retention_period: Some(state::DEFAULT_SNAPSHOT_COPY_RETENTION.to_string()),
                manual_snapshot_retention_period: Some(
                    state::MANUAL_RETENTION_INDEFINITE.to_string()
                ),
                snapshot_copy_grant_name: None,
            }),
            "case {case}: expected the first configuration to stand, got {:?}",
            copy_status(&read.body)
        );
    }

    // A grant name that identifies none of this account's grants: one never
    // created, one created and deleted, and the empty name.
    create_grant(&emulator, GRANT).await;
    let deleted = emulator
        .call(
            "DeleteSnapshotCopyGrant",
            &[("SnapshotCopyGrantName", GRANT)],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case the grant delete: expected it served, got {} with body {}",
        deleted.status, deleted.body
    );
    available_cluster(&emulator, "grant-cluster").await;
    for (case, grant) in [
        ("a grant nothing holds", "no-such-grant"),
        ("a deleted grant", GRANT),
        ("the empty name", ""),
    ] {
        let answer = enable(
            &emulator,
            "grant-cluster",
            "us-east-1",
            &[("SnapshotCopyGrantName", grant)],
        )
        .await;
        assert_refused(case, &answer, 400, "SnapshotCopyGrantNotFoundFault");
        let read = describe(&emulator, "grant-cluster").await;
        assert_eq!(
            copy_status(&read.body),
            None,
            "case {case}: expected the refusal to configure nothing, got {:?}",
            copy_status(&read.body)
        );
    }
}
