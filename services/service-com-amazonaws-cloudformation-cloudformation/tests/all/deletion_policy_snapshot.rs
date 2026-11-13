// SPDX-License-Identifier: CC0-1.0

//! `DeletionPolicy` on a resource whose owning service takes its snapshot as a CALL OF ITS
//! OWN: what a stack's deletion leaves behind in that service.
//!
//! RULE: "With the `DeletionPolicy` attribute you can preserve, and in some cases, backup a
//! resource when its stack is deleted", and for each of the values the attribute takes:
//!
//! * `Snapshot` — "for resources that support snapshots, CloudFormation creates a snapshot
//!   for the resource before deleting it": the resource is gone and EXACTLY ONE snapshot of
//!   it is left behind, held by the service that owns the resource and reported by that
//!   service's own reads;
//! * `Delete` — and a declaration that states no policy, which is the documented default for
//!   a resource of this kind — "CloudFormation deletes the resource and all its content":
//!   the resource is gone and no snapshot of it exists;
//! * `Retain` — "CloudFormation keeps the resource without deleting the resource or its
//!   contents": the resource is still there afterwards, and no snapshot was taken.
//!
//! `AWS::EC2::Volume` is one of the published snapshot-supporting types whose snapshot is NOT
//! a member of its delete call: `ec2:DeleteVolume` takes only `VolumeId`, so the snapshot is
//! an `ec2:CreateSnapshot` of the volume and the volume's size and encryption are read off
//! the volume itself. Every case therefore reads the outcome back through Amazon EC2's own
//! wire — `DescribeSnapshots` filtered on the source volume and `DescribeVolumes` on the
//! volume id — so what it asserts is what a client sees, and no assertion is made against
//! this crate's own store.
//!
//! The rows span the whole catalog of the attribute's values, on the same declaration, and
//! the snapshot row is asserted on both halves of the policy: the data that survives and the
//! resource that does not.
//!
//! The second suite holds the OTHER half of the same rule: the snapshot a deletion leaves is
//! a snapshot OF THAT RESOURCE, so every member the owning service reports for it is read off
//! the resource the snapshot was taken of (its id, its size, its encryption state) and off
//! that service's own clock (the state, the progress and the start time) — never off a
//! default one resource of a stack happens to share with another. Its rows are volumes of
//! one stack that DIFFER along those facts, so a snapshot reported with another volume's
//! size, another volume's encryption state or an id nothing was taken of fails a row.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use service_com_amazonaws_ec2_amazonec2::state::{
    SNAPSHOT_PROGRESS_COMPLETED, SNAPSHOT_STATE_COMPLETED, SNAPSHOT_STATE_PENDING,
    TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, ec2, member, router, wait_for_the_transition, xml_text, xml_texts,
    REGION,
};
use crate::update_rollback_recovery::status_of;

/// The size the volume every row declares asks for, in GiB — which is what its snapshot
/// reports as `volumeSize`.
const VOLUME_SIZE: &str = "1";

/// One row: the `DeletionPolicy` the declaration carries, and what the stack's deletion has
/// to leave behind in Amazon EC2.
struct DeletionPolicyCase {
    case: &'static str,
    stack: &'static str,
    /// The attribute the declaration carries, or `None` for one that states none.
    policy: Option<&'static str>,
    /// How many snapshots of the volume the deletion must leave.
    snapshots: usize,
    /// Whether the volume itself is still there afterwards.
    keeps_the_volume: bool,
}

/// The class the rule covers: the whole catalog of `DeletionPolicy` values a declaration can
/// carry, from the attribute's own documentation.
fn deletion_policy_cases() -> Vec<DeletionPolicyCase> {
    vec![
        DeletionPolicyCase {
            case: "DeletionPolicy: Snapshot",
            stack: "policy-snapshot",
            policy: Some("Snapshot"),
            snapshots: 1,
            keeps_the_volume: false,
        },
        DeletionPolicyCase {
            case: "DeletionPolicy: Delete",
            stack: "policy-delete",
            policy: Some("Delete"),
            snapshots: 0,
            keeps_the_volume: false,
        },
        DeletionPolicyCase {
            case: "no DeletionPolicy at all, which is the documented default",
            stack: "policy-absent",
            policy: None,
            snapshots: 0,
            keeps_the_volume: false,
        },
        DeletionPolicyCase {
            case: "DeletionPolicy: Retain",
            stack: "policy-retain",
            policy: Some("Retain"),
            snapshots: 0,
            keeps_the_volume: true,
        },
    ]
}

/// The template a row deploys: one EBS volume, with the row's policy where it states one.
fn template(policy: Option<&str>) -> String {
    let mut volume = json!({
        "Type": "AWS::EC2::Volume",
        "Properties": {
            "AvailabilityZone": format!("{REGION}a"),
            "Size": VOLUME_SIZE,
            "VolumeType": "gp3",
        },
    });
    if let Some(policy) = policy {
        volume["DeletionPolicy"] = json!(policy);
    }
    json!({"Resources": {"Vol": volume}}).to_string()
}

/// The `PhysicalResourceId` a stack reports for one of its resources.
async fn physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical: &str,
) -> String {
    let (status_code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStackResource to succeed, got {status_code} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// The snapshots Amazon EC2 holds OF one volume, as `(snapshotId, volumeId, volumeSize)` —
/// read through the filtered listing a client uses, so the answer is that service's own.
async fn snapshots_of(
    router: &MultiServiceRouter,
    case: &str,
    volume_id: &str,
) -> Vec<(String, String, String)> {
    let (status_code, body) = ec2(
        router,
        "DescribeSnapshots",
        &[
            ("Owner.1", "self"),
            ("Filter.1.Name", "volume-id"),
            ("Filter.1.Value.1", volume_id),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeSnapshots to succeed, got {status_code} {body}"
    );
    let ids = xml_texts(&body, "snapshotId");
    let volumes = xml_texts(&body, "volumeId");
    let sizes = xml_texts(&body, "volumeSize");
    ids.into_iter()
        .zip(volumes)
        .zip(sizes)
        .map(|((id, volume), size)| (id, volume, size))
        .collect()
}

#[tokio::test]
async fn a_stack_deletion_leaves_the_snapshot_its_deletion_policy_asks_for() {
    for row in deletion_policy_cases() {
        let case = row.case;
        let router = router();
        let body_of_the_template = template(row.policy);
        let (status_code, body) =
            create_stack(&router, row.stack, &body_of_the_template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to succeed, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let created = status_of(&router, row.stack).await;
        assert_eq!(
            created,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create to complete, got {created:?}"
        );
        let volume_id = physical_id(&router, case, row.stack, "Vol").await;
        assert!(
            volume_id.starts_with("vol-"),
            "case {case:?}: expected the stack to report the volume EC2 minted, got \
             {volume_id:?}"
        );
        // The baseline: no snapshot of the volume exists before the deletion, whatever the
        // policy says — the snapshot is the DELETION's, not the create's.
        let before = snapshots_of(&router, case, &volume_id).await;
        assert!(
            before.is_empty(),
            "case {case:?}: expected no snapshot of {volume_id} before the deletion, got \
             {before:?}"
        );

        let (status_code, body) =
            call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DeleteStack to succeed, got {status_code} {body}"
        );
        wait_for_the_transition().await;

        // What the deletion left in the service that OWNS the data: the snapshots the policy
        // asked for, and no more than those.
        let after = snapshots_of(&router, case, &volume_id).await;
        assert_eq!(
            after.len(),
            row.snapshots,
            "case {case:?}: expected the deletion to leave {} snapshot(s) of {volume_id}, got \
             {after:?}",
            row.snapshots
        );
        for (snapshot_id, snapshot_volume, snapshot_size) in &after {
            assert!(
                snapshot_id.starts_with("snap-"),
                "case {case:?}: expected the snapshot to carry the id EC2 minted, got \
                 {snapshot_id:?}"
            );
            assert_eq!(
                snapshot_volume, &volume_id,
                "case {case:?}: expected the snapshot to name the volume it was taken of, got \
                 {snapshot_volume:?}"
            );
            assert_eq!(
                snapshot_size, VOLUME_SIZE,
                "case {case:?}: expected the snapshot to report the volume's own size, got \
                 {snapshot_size:?}"
            );
            // ...and it is addressable by its own id, which is what a restore flow reads it
            // back with.
            let (status_code, body) =
                ec2(&router, "DescribeSnapshots", &[("SnapshotId.1", snapshot_id)]).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the snapshot to be addressable by its id, got \
                 {status_code} {body}"
            );
            let reported = xml_text(&body, "volumeId").unwrap_or_default();
            assert_eq!(
                &reported, &volume_id,
                "case {case:?}: expected the snapshot read by id to name the same volume, got \
                 {reported:?}"
            );
        }

        // ...and what it left of the VOLUME: `Retain` keeps it, every other policy takes it
        // away.
        let (status_code, body) = ec2(&router, "DescribeVolumes", &[("VolumeId.1", &volume_id)])
            .await;
        if row.keeps_the_volume {
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the retained volume to still be held by EC2, got \
                 {status_code} {body}"
            );
        } else {
            assert_eq!(
                status_code, 400,
                "case {case:?}: expected EC2 to hold no volume for {volume_id} after the \
                 deletion, got {status_code} {body}"
            );
            let code = xml_text(&body, "Code").unwrap_or_default();
            assert_eq!(
                code, "InvalidVolume.NotFound",
                "case {case:?}: expected the deleted volume to be refused as not found, got \
                 {code:?} {body}"
            );
        }
    }
}

/// One EBS volume of the estate the second suite deploys: the facts of the VOLUME, which are
/// what a snapshot of it has to report, and the policy that decides whether a snapshot of it
/// is taken at all.
struct VolumeRow {
    case: &'static str,
    /// The logical id the volume is declared under, which its physical id is read with.
    logical: &'static str,
    /// `Size`, in GiB — inside the range the volume type publishes (source: `CreateVolume`'s
    /// `Size` member, "gp2: 1 - 16,384 GiB gp3: 1 - 65,536 GiB").
    size: &'static str,
    /// `VolumeType` — one of the catalog `CreateVolume`'s `VolumeType` admits.
    volume_type: &'static str,
    /// `Encrypted`, as the declaration states it: "snapshots that are taken from encrypted
    /// volumes are automatically encrypted" (`Snapshot$Encrypted`).
    encrypted: bool,
    /// The `DeletionPolicy` the declaration carries.
    policy: &'static str,
    /// How many snapshots of this volume the stack's deletion must leave.
    snapshots: usize,
}

/// The volumes one stack declares, differing along exactly what the rule generalises over:
/// the size, the volume type, the encryption state and the policy — so a snapshot reported
/// with a size or an encryption state that belongs to a SIBLING volume fails a row.
fn volume_rows() -> Vec<VolumeRow> {
    vec![
        VolumeRow {
            case: "a 1 GiB unencrypted gp3 volume under Snapshot",
            logical: "Smallest",
            size: "1",
            volume_type: "gp3",
            encrypted: false,
            policy: "Snapshot",
            snapshots: 1,
        },
        VolumeRow {
            case: "an 8 GiB encrypted gp3 volume under Snapshot",
            logical: "Encrypted",
            size: "8",
            volume_type: "gp3",
            encrypted: true,
            policy: "Snapshot",
            snapshots: 1,
        },
        VolumeRow {
            case: "a 400 GiB unencrypted gp2 volume under Snapshot",
            logical: "Larger",
            size: "400",
            volume_type: "gp2",
            encrypted: false,
            policy: "Snapshot",
            snapshots: 1,
        },
        VolumeRow {
            case: "a 3 GiB encrypted gp3 volume under Retain, beside the snapshotted ones",
            logical: "Kept",
            size: "3",
            volume_type: "gp3",
            encrypted: true,
            policy: "Retain",
            snapshots: 0,
        },
    ]
}

/// The template that deploys the whole estate: one `AWS::EC2::Volume` per row, each with its
/// own facts and its own policy.
fn estate_template(rows: &[VolumeRow]) -> String {
    let mut resources = serde_json::Map::new();
    for row in rows {
        resources.insert(
            row.logical.to_string(),
            json!({
                "Type": "AWS::EC2::Volume",
                "DeletionPolicy": row.policy,
                "Properties": {
                    "AvailabilityZone": format!("{REGION}a"),
                    "Size": row.size,
                    "VolumeType": row.volume_type,
                    "Encrypted": row.encrypted,
                },
            }),
        );
    }
    json!({"Resources": resources}).to_string()
}

/// The facts Amazon EC2 reports of ONE volume: what a snapshot of it must carry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VolumeFacts {
    size: String,
    encrypted: String,
}

/// The members Amazon EC2 reports for ONE snapshot, as the text the answer carried for each.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedSnapshot {
    snapshot_id: String,
    volume_id: String,
    volume_size: String,
    encrypted: String,
    owner_id: String,
    state: String,
    progress: String,
    start_time: String,
    description: String,
}

/// Every member of the one snapshot an answer reported, read off that answer.
fn reported_snapshot(body: &str) -> ReportedSnapshot {
    let text = |tag: &str| xml_text(body, tag).unwrap_or_default();
    ReportedSnapshot {
        snapshot_id: text("snapshotId"),
        volume_id: text("volumeId"),
        volume_size: text("volumeSize"),
        encrypted: text("encrypted"),
        owner_id: text("ownerId"),
        state: text("status"),
        progress: text("progress"),
        start_time: text("startTime"),
        description: text("description"),
    }
}

/// What Amazon EC2 reports of a volume it holds, read through its own `DescribeVolumes`.
async fn volume_facts(router: &MultiServiceRouter, case: &str, volume_id: &str) -> VolumeFacts {
    let (status_code, body) = ec2(router, "DescribeVolumes", &[("VolumeId.1", volume_id)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeVolumes({volume_id}) to succeed, got {status_code} \
         {body}"
    );
    VolumeFacts {
        size: xml_text(&body, "size").unwrap_or_default(),
        encrypted: xml_text(&body, "encrypted").unwrap_or_default(),
    }
}

/// The one snapshot Amazon EC2 reports for `snapshot_id`, with every member it carries.
async fn snapshot_by_id(
    router: &MultiServiceRouter,
    case: &str,
    snapshot_id: &str,
) -> ReportedSnapshot {
    let (status_code, body) = ec2(
        router,
        "DescribeSnapshots",
        &[("SnapshotId.1", snapshot_id)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeSnapshots({snapshot_id}) to succeed, got {status_code} \
         {body}"
    );
    reported_snapshot(&body)
}

/// The snapshot ids the account holds, read through the listing a client polls with —
/// `OwnerIds=["self"]` and nothing else.
async fn every_snapshot_of_the_account(router: &MultiServiceRouter, case: &str) -> BTreeSet<String> {
    let (status_code, body) = ec2(router, "DescribeSnapshots", &[("Owner.1", "self")]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected the account-wide DescribeSnapshots to succeed, got \
         {status_code} {body}"
    );
    xml_texts(&body, "snapshotId").into_iter().collect()
}

#[tokio::test]
async fn each_snapshot_a_deletion_leaves_reports_the_facts_of_its_own_volume() {
    let rows = volume_rows();
    let router = router();
    let stack = "policy-snapshot-estate";
    let body_of_the_template = estate_template(&rows);
    let (status_code, created) = create_stack(&router, stack, &body_of_the_template, &[]).await;
    assert_eq!(
        status_code, 200,
        "expected CreateStack to succeed, got {status_code} {created}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let created_status = status_of(&router, stack).await;
    assert_eq!(
        created_status,
        status::CREATE_COMPLETE,
        "expected the create to complete, got {created_status:?}"
    );

    // Each row's own volume, and the facts Amazon EC2 holds of it: the snapshot's members are
    // asserted against THESE rather than against the template, so what is checked is that the
    // snapshot was taken of that volume as that service holds it.
    let mut volumes: Vec<(String, VolumeFacts)> = Vec::new();
    for row in &rows {
        let case = row.case;
        let volume_id = physical_id(&router, case, stack, row.logical).await;
        assert!(
            volume_id.starts_with("vol-"),
            "case {case:?}: expected the stack to report the volume EC2 minted, got {volume_id:?}"
        );
        let facts = volume_facts(&router, case, &volume_id).await;
        assert_eq!(
            facts.size, row.size,
            "case {case:?}: expected EC2 to hold the declared size for {volume_id}, got {facts:?}"
        );
        assert_eq!(
            facts.encrypted,
            row.encrypted.to_string(),
            "case {case:?}: expected EC2 to hold the declared encryption state for {volume_id}, \
             got {facts:?}"
        );
        let before = snapshots_of(&router, case, &volume_id).await;
        assert!(
            before.is_empty(),
            "case {case:?}: expected no snapshot of {volume_id} before the deletion, got \
             {before:?}"
        );
        volumes.push((volume_id, facts));
    }
    let distinct: BTreeSet<&String> = volumes.iter().map(|(id, _)| id).collect();
    assert_eq!(
        distinct.len(),
        rows.len(),
        "expected one volume of its own per declaration, got {volumes:?}"
    );

    let (status_code, body) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "expected DeleteStack to succeed, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    // What the deletion left of each volume, read through the filtered listing a client uses.
    let mut left_behind: BTreeSet<String> = BTreeSet::new();
    for (row, (volume_id, facts)) in rows.iter().zip(&volumes) {
        let case = row.case;
        let after = snapshots_of(&router, case, volume_id).await;
        assert_eq!(
            after.len(),
            row.snapshots,
            "case {case:?}: expected the deletion to leave {} snapshot(s) of {volume_id}, got \
             {after:?}",
            row.snapshots
        );
        for (snapshot_id, _, _) in &after {
            let snapshot = snapshot_by_id(&router, case, snapshot_id).await;
            assert_eq!(
                snapshot.volume_id, *volume_id,
                "case {case:?}: expected the snapshot to name the volume it was taken of, got \
                 {snapshot:?}"
            );
            assert_eq!(
                snapshot.volume_size, facts.size,
                "case {case:?}: expected the snapshot to report {volume_id}'s own size \
                 ({}), got {snapshot:?}",
                facts.size
            );
            assert_eq!(
                snapshot.encrypted, facts.encrypted,
                "case {case:?}: expected the snapshot to report {volume_id}'s own encryption \
                 state ({}), got {snapshot:?}",
                facts.encrypted
            );
            assert_eq!(
                snapshot.owner_id,
                engine::emulator_account_id(),
                "case {case:?}: expected the snapshot to be owned by the calling account, got \
                 {snapshot:?}"
            );
            // The snapshot is in a state of the clock's catalog — never `error`, which is the
            // state that says the data was NOT preserved — and carries the instant it started.
            assert!(
                snapshot.state == SNAPSHOT_STATE_PENDING
                    || snapshot.state == SNAPSHOT_STATE_COMPLETED,
                "case {case:?}: expected the snapshot to be {SNAPSHOT_STATE_PENDING:?} or \
                 {SNAPSHOT_STATE_COMPLETED:?}, got {snapshot:?}"
            );
            assert!(
                snapshot.start_time.contains('T'),
                "case {case:?}: expected the snapshot to carry the instant it started, got \
                 {snapshot:?}"
            );
            // ...and it names the resource whose contents it holds, which is the provenance
            // CloudFormation asks the owning service for the snapshot under.
            assert!(
                snapshot.description.starts_with(&format!("{volume_id}-")),
                "case {case:?}: expected the snapshot's description to name {volume_id}, got \
                 {snapshot:?}"
            );
            left_behind.insert(snapshot_id.clone());
        }
    }

    // Exactly those snapshots and no others: a listing of the whole account reports the ones
    // the policies asked for, so no snapshot was invented for a volume no policy covered.
    let held = every_snapshot_of_the_account(&router, "the whole account").await;
    assert_eq!(
        held, left_behind,
        "expected the account to hold exactly the snapshots the policies asked for, got {held:?}"
    );

    // The CLOCK, not a caller, completes each snapshot: every one of them reports the same
    // volume afterwards (a read changes nothing) and the completed state and progress once the
    // move the clock owed is due.
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
    for snapshot_id in &left_behind {
        let before = snapshot_by_id(&router, "past the transition", snapshot_id).await;
        let again = snapshot_by_id(&router, "past the transition", snapshot_id).await;
        assert_eq!(
            before, again,
            "case {snapshot_id:?}: expected two reads to agree, got {before:?} and {again:?}"
        );
        assert_eq!(
            again.state, SNAPSHOT_STATE_COMPLETED,
            "case {snapshot_id:?}: expected the snapshot to have completed, got {again:?}"
        );
        assert_eq!(
            again.progress, SNAPSHOT_PROGRESS_COMPLETED,
            "case {snapshot_id:?}: expected the completed snapshot's progress, got {again:?}"
        );
    }

    // ...and the volume the `Retain` row kept is still there, while every other row's is gone.
    for (row, (volume_id, _)) in rows.iter().zip(&volumes) {
        let case = row.case;
        let (status_code, body) =
            ec2(&router, "DescribeVolumes", &[("VolumeId.1", volume_id)]).await;
        if row.policy == "Retain" {
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the retained volume to still be held by EC2, got \
                 {status_code} {body}"
            );
        } else {
            assert_eq!(
                status_code, 400,
                "case {case:?}: expected EC2 to hold no volume for {volume_id} after the \
                 deletion, got {status_code} {body}"
            );
            let code = xml_text(&body, "Code").unwrap_or_default();
            assert_eq!(
                code, "InvalidVolume.NotFound",
                "case {case:?}: expected the deleted volume to be refused as not found, got \
                 {code:?} {body}"
            );
        }
    }
}
