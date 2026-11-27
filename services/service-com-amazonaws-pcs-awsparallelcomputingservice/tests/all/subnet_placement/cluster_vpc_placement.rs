// SPDX-License-Identifier: CC0-1.0

//! Where a compute node group may be placed: the VPC its CLUSTER is in.
//!
//! The rule under test: a subnet a request places compute nodes in must belong to the same
//! VPC as the cluster those nodes join — "The subnets must be in the same VPC as the
//! cluster" (`CreateComputeNodeGroupRequest$subnetIds` and
//! `UpdateComputeNodeGroupRequest$subnetIds`) — because an instance in another VPC could
//! never reach the controller the cluster's own subnets hold. The VPC a request is judged
//! against is whichever one the cluster was created over, so it is read from the cluster's
//! stored `networking.subnetIds` rather than from anything the request asserts, and both
//! sides of the comparison come from EC2, the owner of the account's subnets.
//!
//! The rows span what the rule generalises over: how many subnets a request names, which
//! VPC each one is in, the ORDER an outside subnet appears in, how many subnets the cluster
//! itself spans, which cluster (so which VPC) the group is created under or updated under,
//! and which operation — the create or the update — named them. Existence is the earlier
//! question, so an id EC2 reports no subnet for is still answered with the absent sentence,
//! which has no VPC to compare, and a subnet that stops existing stops resolving. And
//! nothing is assumed from silence: with no EC2 to ask, no placement is refused.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::{
    create_cluster, create_subnet, create_vpc, emulator_with_ec2, unresolvable_subnet,
    SUBNETS_ABSENT, VPC_CIDR,
};
use crate::cluster_lifecycle::{call, emulator};

/// What PCS answers a request that would place compute nodes outside the cluster's VPC. The
/// whole sentence is fixed text: it names no subnet id and no VPC id.
const SUBNETS_OUTSIDE_CLUSTER_VPC: &str = "The subnets must be in the same VPC as the cluster.";

/// The CIDR of the SECOND VPC these cases carve subnets out of — the one no cluster of the
/// case under test was created over.
const OTHER_VPC_CIDR: &str = "10.99.0.0/16";

/// A `CreateComputeNodeGroup` request placing `name` in `subnets`, under `cluster`.
fn create_group(cluster: &str, name: &str, subnets: &[String]) -> Value {
    json!({
        "clusterIdentifier": cluster,
        "computeNodeGroupName": name,
        "subnetIds": subnets,
        "customLaunchTemplate": {"id": "lt-0123456789abcdef0", "version": "1"},
        "iamInstanceProfileArn": format!(
            "arn:aws:iam::{}:instance-profile/AWSPCS-demo",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "scalingConfiguration": {"minInstanceCount": 0, "maxInstanceCount": 4},
        "instanceConfigs": [{"instanceType": "c6i.large"}],
    })
}

/// How many compute node groups `cluster` holds.
async fn group_count(router: &MultiServiceRouter, cluster: &str) -> usize {
    let (status, listed) = call(
        router,
        "ListComputeNodeGroups",
        &json!({"clusterIdentifier": cluster}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case ListComputeNodeGroups({cluster}): got {listed}"
    );
    listed["computeNodeGroups"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// One row of [`an_update_is_judged_against_the_whole_vpc_the_cluster_spans`]: a case name,
/// the subnets the update names (`None` carries no `subnetIds` at all), and the sentence it
/// is refused with (`None` when it is served).
type UpdateRow = (&'static str, Option<Vec<String>>, Option<&'static str>);

/// An `UpdateComputeNodeGroup` request about `group` in `cluster`, placing it in `subnets`
/// when the member is carried at all. `None` omits `subnetIds` and moves another member
/// instead, so the request is a real change with no placement for the networking gate to
/// judge.
fn update_group(cluster: &str, group: &str, subnets: Option<&[String]>) -> Value {
    let mut request = json!({
        "clusterIdentifier": cluster,
        "computeNodeGroupIdentifier": group,
    });
    match subnets {
        Some(subnets) => request["subnetIds"] = json!(subnets),
        // Any id of the form `AmiId`'s `@pattern` admits serves: what this arm is for is a
        // request that changes the group without naming a placement at all.
        None => request["amiId"] = json!("ami-00f1e2d3c4b5a6978"),
    }
    request
}

/// The subnets `GetComputeNodeGroup` reports for a stored group.
async fn stored_subnets(router: &MultiServiceRouter, cluster: &str, group: &str) -> Value {
    let (status, read) = call(
        router,
        "GetComputeNodeGroup",
        &json!({"clusterIdentifier": cluster, "computeNodeGroupIdentifier": group}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case GetComputeNodeGroup({cluster}/{group}): got {read}"
    );
    read["computeNodeGroup"]["subnetIds"].clone()
}

#[tokio::test]
async fn a_create_places_a_group_only_in_the_clusters_own_vpc() {
    let (router, ec2) = emulator_with_ec2();
    let cluster_vpc = create_vpc(&ec2, VPC_CIDR).await;
    let inside_one = create_subnet(&ec2, &cluster_vpc, "10.42.1.0/24").await;
    let inside_two = create_subnet(&ec2, &cluster_vpc, "10.42.2.0/24").await;
    let other_vpc = create_vpc(&ec2, OTHER_VPC_CIDR).await;
    let outside_one = create_subnet(&ec2, &other_vpc, "10.99.1.0/24").await;
    let outside_two = create_subnet(&ec2, &other_vpc, "10.99.2.0/24").await;

    // The cluster's control plane is in ONE of the first VPC's subnets, which is what makes
    // that VPC the cluster's.
    let (status, created) = call(
        &router,
        "CreateCluster",
        &create_cluster("vpc-host", "24.11", std::slice::from_ref(&inside_one)),
    )
    .await;
    assert_eq!(status, 200, "case create the cluster: got {created}");

    // (case, the subnets the group names, the sentence it is refused with or None)
    let rows: Vec<(&str, Vec<String>, Option<&str>)> = vec![
        (
            "the cluster's own subnet",
            vec![inside_one.clone()],
            None,
        ),
        (
            "another subnet of the cluster's VPC",
            vec![inside_two.clone()],
            None,
        ),
        (
            "two subnets of the cluster's VPC",
            vec![inside_one.clone(), inside_two.clone()],
            None,
        ),
        (
            "one subnet of another VPC",
            vec![outside_one.clone()],
            Some(SUBNETS_OUTSIDE_CLUSTER_VPC),
        ),
        (
            "two subnets of another VPC",
            vec![outside_one.clone(), outside_two.clone()],
            Some(SUBNETS_OUTSIDE_CLUSTER_VPC),
        ),
        (
            "the cluster's subnet and one of another VPC",
            vec![inside_one.clone(), outside_one.clone()],
            Some(SUBNETS_OUTSIDE_CLUSTER_VPC),
        ),
        (
            "a subnet of another VPC ahead of the cluster's",
            vec![outside_two.clone(), inside_two.clone()],
            Some(SUBNETS_OUTSIDE_CLUSTER_VPC),
        ),
        (
            // Existence is the earlier question: an id that names no subnet has no VPC.
            "an id EC2 holds no subnet for",
            vec![unresolvable_subnet(21)],
            Some(SUBNETS_ABSENT),
        ),
        (
            "an unresolvable id beside a subnet of another VPC",
            vec![unresolvable_subnet(22), outside_one.clone()],
            Some(SUBNETS_ABSENT),
        ),
    ];

    let mut served = 0_usize;
    for (index, (case, subnets, refusal)) in rows.iter().enumerate() {
        let name = format!("nodes-{index}");
        let (status, body) = call(
            &router,
            "CreateComputeNodeGroup",
            &create_group("vpc-host", &name, subnets),
        )
        .await;
        match refusal {
            None => {
                served += 1;
                assert_eq!(
                    status, 200,
                    "case {case}: expected the create to be served, got {body}"
                );
                assert_eq!(
                    body["computeNodeGroup"]["subnetIds"],
                    json!(subnets),
                    "case {case}: expected the placement to persist, got {body}"
                );
                assert_eq!(
                    stored_subnets(&router, "vpc-host", &name).await,
                    json!(subnets),
                    "case {case}: expected the stored placement on the read back"
                );
            }
            Some(sentence) => {
                assert_eq!(
                    status, 400,
                    "case {case}: expected the create to be refused, got {body}"
                );
                assert_eq!(
                    body["__type"],
                    json!("ValidationException"),
                    "case {case}: got {body}"
                );
                assert_eq!(
                    body["message"],
                    json!(sentence),
                    "case {case}: got {body}"
                );
                for id in subnets {
                    assert!(
                        !body["message"]
                            .as_str()
                            .unwrap_or_default()
                            .contains(id.as_str()),
                        "case {case}: the sentence must name no subnet id, got {body}"
                    );
                }
            }
        }
        let held = group_count(&router, "vpc-host").await;
        assert_eq!(
            held, served,
            "case {case}: expected only the served creates to have persisted, got {held}"
        );
    }
}

#[tokio::test]
async fn each_cluster_bounds_its_groups_to_the_vpc_it_was_created_over() {
    let (router, ec2) = emulator_with_ec2();
    let first_vpc = create_vpc(&ec2, VPC_CIDR).await;
    let in_first = create_subnet(&ec2, &first_vpc, "10.42.6.0/24").await;
    let second_vpc = create_vpc(&ec2, OTHER_VPC_CIDR).await;
    let in_second = create_subnet(&ec2, &second_vpc, "10.99.6.0/24").await;
    let also_in_second = create_subnet(&ec2, &second_vpc, "10.99.7.0/24").await;

    for (cluster, subnet) in [("first-host", &in_first), ("second-host", &in_second)] {
        let (status, created) = call(
            &router,
            "CreateCluster",
            &create_cluster(cluster, "24.11", std::slice::from_ref(subnet)),
        )
        .await;
        assert_eq!(status, 200, "case create {cluster}: got {created}");
    }

    // (case, the cluster the group is created under, the subnet it names, whether it is served)
    let rows: [(&str, &str, &String, bool); 4] = [
        ("the first cluster in its own VPC", "first-host", &in_first, true),
        (
            "the first cluster in the second VPC",
            "first-host",
            &also_in_second,
            false,
        ),
        (
            "the second cluster in its own VPC",
            "second-host",
            &also_in_second,
            true,
        ),
        (
            "the second cluster in the first VPC",
            "second-host",
            &in_first,
            false,
        ),
    ];

    for (index, (case, cluster, subnet, is_served)) in rows.iter().enumerate() {
        let name = format!("group-{index}");
        let (status, body) = call(
            &router,
            "CreateComputeNodeGroup",
            &create_group(cluster, &name, std::slice::from_ref(*subnet)),
        )
        .await;
        if *is_served {
            assert_eq!(status, 200, "case {case}: expected it served, got {body}");
            assert_eq!(
                stored_subnets(&router, cluster, &name).await,
                json!([subnet]),
                "case {case}: expected the stored placement on the read back"
            );
        } else {
            assert_eq!(status, 400, "case {case}: expected it refused, got {body}");
            assert_eq!(
                body["message"],
                json!(SUBNETS_OUTSIDE_CLUSTER_VPC),
                "case {case}: got {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_cannot_move_a_group_out_of_the_clusters_vpc() {
    let (router, ec2) = emulator_with_ec2();
    let cluster_vpc = create_vpc(&ec2, VPC_CIDR).await;
    let inside_one = create_subnet(&ec2, &cluster_vpc, "10.42.8.0/24").await;
    let inside_two = create_subnet(&ec2, &cluster_vpc, "10.42.9.0/24").await;
    let other_vpc = create_vpc(&ec2, OTHER_VPC_CIDR).await;
    let outside = create_subnet(&ec2, &other_vpc, "10.99.8.0/24").await;

    let (status, created) = call(
        &router,
        "CreateCluster",
        &create_cluster("update-host", "24.11", std::slice::from_ref(&inside_one)),
    )
    .await;
    assert_eq!(status, 200, "case create the cluster: got {created}");
    let (status, created) = call(
        &router,
        "CreateComputeNodeGroup",
        &create_group("update-host", "movers", std::slice::from_ref(&inside_one)),
    )
    .await;
    assert_eq!(status, 200, "case create the group: got {created}");

    // (case, the subnets the update names, the sentence it is refused with or None)
    let rows: Vec<(&str, Vec<String>, Option<&str>)> = vec![
        (
            "a subnet of another VPC",
            vec![outside.clone()],
            Some(SUBNETS_OUTSIDE_CLUSTER_VPC),
        ),
        (
            "the cluster's subnet and one of another VPC",
            vec![inside_two.clone(), outside.clone()],
            Some(SUBNETS_OUTSIDE_CLUSTER_VPC),
        ),
        (
            "an id EC2 holds no subnet for",
            vec![unresolvable_subnet(31)],
            Some(SUBNETS_ABSENT),
        ),
        (
            "another subnet of the cluster's VPC",
            vec![inside_two.clone()],
            None,
        ),
        (
            "both subnets of the cluster's VPC",
            vec![inside_one.clone(), inside_two.clone()],
            None,
        ),
    ];

    // What the group is placed in before each row, so a refused update is shown to have
    // changed nothing.
    let mut placed = json!([inside_one.clone()]);
    for (case, subnets, refusal) in &rows {
        let (status, body) = call(
            &router,
            "UpdateComputeNodeGroup",
            &update_group("update-host", "movers", Some(subnets)),
        )
        .await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: expected the update to be served, got {body}"
                );
                placed = json!(subnets);
            }
            Some(sentence) => {
                assert_eq!(
                    status, 400,
                    "case {case}: expected the update to be refused, got {body}"
                );
                assert_eq!(
                    body["__type"],
                    json!("ValidationException"),
                    "case {case}: got {body}"
                );
                assert_eq!(
                    body["message"],
                    json!(sentence),
                    "case {case}: got {body}"
                );
            }
        }
        assert_eq!(
            stored_subnets(&router, "update-host", "movers").await,
            placed,
            "case {case}: expected the stored placement to be {placed}"
        );
    }
}

#[tokio::test]
async fn a_placement_no_ec2_answered_for_is_refused_nothing() {
    // No EC2 is mounted here, so nothing is known about which VPC any subnet is in — and a
    // refusal is grounded in what EC2 answered, never assumed from silence.
    let router = emulator();
    let (status, created) = call(
        &router,
        "CreateCluster",
        &create_cluster("no-peer-host", "24.11", &[unresolvable_subnet(41)]),
    )
    .await;
    assert_eq!(status, 200, "case create the cluster: got {created}");

    let (status, served) = call(
        &router,
        "CreateComputeNodeGroup",
        &create_group("no-peer-host", "unknowable", &[unresolvable_subnet(42)]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case no EC2 mounted: expected the create to be served, got {served}"
    );
    assert_eq!(
        served["computeNodeGroup"]["subnetIds"],
        json!([unresolvable_subnet(42)]),
        "case no EC2 mounted: expected the placement to persist, got {served}"
    );

    let (status, updated) = call(
        &router,
        "UpdateComputeNodeGroup",
        &update_group(
            "no-peer-host",
            "unknowable",
            Some(&[unresolvable_subnet(43)]),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case no EC2 mounted, on the update: expected it served, got {updated}"
    );
    assert_eq!(
        stored_subnets(&router, "no-peer-host", "unknowable").await,
        json!([unresolvable_subnet(43)]),
        "case no EC2 mounted, on the update: expected the new placement stored"
    );
}

/// The dimensions this case adds to
/// [`an_update_cannot_move_a_group_out_of_the_clusters_vpc`] above, which already covers an
/// update into another VPC on a cluster whose control plane sits in ONE subnet: a cluster
/// that spans TWO subnets of its VPC, a subnet of that VPC the control plane does not sit
/// in, three subnets at once, an outside subnet FIRST in the list, an unresolvable id beside
/// a resolvable one, an update that carries no `subnetIds` at all, and a subnet of the
/// cluster's own VPC that stops existing.
#[tokio::test]
async fn an_update_is_judged_against_the_whole_vpc_the_cluster_spans() {
    let (router, ec2) = emulator_with_ec2();
    let cluster_vpc = create_vpc(&ec2, VPC_CIDR).await;
    let control_one = create_subnet(&ec2, &cluster_vpc, "10.42.10.0/24").await;
    let control_two = create_subnet(&ec2, &cluster_vpc, "10.42.11.0/24").await;
    // A subnet of the cluster's VPC that its control plane does NOT sit in: the VPC bounds
    // the placement, not the exact subnet list the cluster was created over.
    let spare = create_subnet(&ec2, &cluster_vpc, "10.42.12.0/24").await;
    let other_vpc = create_vpc(&ec2, OTHER_VPC_CIDR).await;
    let outside = create_subnet(&ec2, &other_vpc, "10.99.10.0/24").await;

    let control_plane = [control_one.clone(), control_two.clone()];
    let (status, created) = call(
        &router,
        "CreateCluster",
        &create_cluster("span-host", "24.11", &control_plane),
    )
    .await;
    assert_eq!(status, 200, "case create the cluster: got {created}");
    let (status, created) = call(
        &router,
        "CreateComputeNodeGroup",
        &create_group("span-host", "spanners", std::slice::from_ref(&control_one)),
    )
    .await;
    assert_eq!(status, 200, "case create the group: got {created}");

    // The read that follows the create observes the group settled, so every row below is
    // decided by its placement rather than by the group's status.
    let mut placed = json!([control_one.clone()]);
    assert_eq!(
        stored_subnets(&router, "span-host", "spanners").await,
        placed,
        "case the created placement: expected {placed}"
    );

    // (case, the subnets the update names — `None` carries no `subnetIds` at all, the
    // sentence it is refused with or None)
    let rows: Vec<UpdateRow> = vec![
        (
            "a subnet of the cluster's VPC its control plane does not sit in",
            Some(vec![spare.clone()]),
            None,
        ),
        (
            "every subnet of the cluster's VPC",
            Some(vec![
                control_one.clone(),
                control_two.clone(),
                spare.clone(),
            ]),
            None,
        ),
        (
            "a subnet of another VPC ahead of one of the cluster's",
            Some(vec![outside.clone(), spare.clone()]),
            Some(SUBNETS_OUTSIDE_CLUSTER_VPC),
        ),
        (
            "two of the cluster's VPC and one outside it last",
            Some(vec![control_two.clone(), spare.clone(), outside.clone()]),
            Some(SUBNETS_OUTSIDE_CLUSTER_VPC),
        ),
        (
            // Existence is the earlier question: an id that names no subnet has no VPC.
            "an id EC2 holds no subnet for beside one of the cluster's VPC",
            Some(vec![unresolvable_subnet(51), control_two.clone()]),
            Some(SUBNETS_ABSENT),
        ),
        (
            "no subnetIds at all — another member moves instead",
            None,
            None,
        ),
    ];

    for (case, subnets, refusal) in &rows {
        let (status, body) = call(
            &router,
            "UpdateComputeNodeGroup",
            &update_group("span-host", "spanners", subnets.as_deref()),
        )
        .await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: expected the update to be served, got {body}"
                );
                if let Some(subnets) = subnets {
                    placed = json!(subnets);
                }
            }
            Some(sentence) => {
                assert_eq!(
                    status, 400,
                    "case {case}: expected the update to be refused, got {body}"
                );
                assert_eq!(
                    body["__type"],
                    json!("ValidationException"),
                    "case {case}: got {body}"
                );
                assert_eq!(
                    body["message"],
                    json!(sentence),
                    "case {case}: got {body}"
                );
            }
        }
        assert_eq!(
            stored_subnets(&router, "span-host", "spanners").await,
            placed,
            "case {case}: expected the stored placement to be {placed}"
        );
    }

    // Nothing is cached, so a subnet of the cluster's own VPC that stops existing stops
    // resolving: the update naming it is answered with the earlier question's sentence.
    ec2.delete_subnet()
        .subnet_id(&spare)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: EC2 must delete the subnet: {error:?}"));
    let (status, refused) = call(
        &router,
        "UpdateComputeNodeGroup",
        &update_group(
            "span-host",
            "spanners",
            Some(std::slice::from_ref(&spare)),
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a deleted subnet of the cluster's VPC: expected it refused, got {refused}"
    );
    assert_eq!(
        refused["message"],
        json!(SUBNETS_ABSENT),
        "case a deleted subnet of the cluster's VPC: got {refused}"
    );
    assert_eq!(
        stored_subnets(&router, "span-host", "spanners").await,
        placed,
        "case a deleted subnet of the cluster's VPC: expected the stored placement to be \
         {placed}"
    );
}

#[tokio::test]
async fn each_cluster_bounds_its_groups_updates_to_the_vpc_it_was_created_over() {
    let (router, ec2) = emulator_with_ec2();
    let first_vpc = create_vpc(&ec2, VPC_CIDR).await;
    let in_first = create_subnet(&ec2, &first_vpc, "10.42.14.0/24").await;
    let also_in_first = create_subnet(&ec2, &first_vpc, "10.42.15.0/24").await;
    let second_vpc = create_vpc(&ec2, OTHER_VPC_CIDR).await;
    let in_second = create_subnet(&ec2, &second_vpc, "10.99.14.0/24").await;
    let also_in_second = create_subnet(&ec2, &second_vpc, "10.99.15.0/24").await;

    // One cluster per VPC, each holding one group placed in its own cluster's subnet.
    for (cluster, group, subnet) in [
        ("first-host", "first-nodes", &in_first),
        ("second-host", "second-nodes", &in_second),
    ] {
        let (status, created) = call(
            &router,
            "CreateCluster",
            &create_cluster(cluster, "24.11", std::slice::from_ref(subnet)),
        )
        .await;
        assert_eq!(status, 200, "case create {cluster}: got {created}");
        let (status, created) = call(
            &router,
            "CreateComputeNodeGroup",
            &create_group(cluster, group, std::slice::from_ref(subnet)),
        )
        .await;
        assert_eq!(status, 200, "case create {group}: got {created}");
        // Settles the group, so the updates below are decided by their placement.
        assert_eq!(
            stored_subnets(&router, cluster, group).await,
            json!([subnet]),
            "case create {group}: expected the created placement"
        );
    }

    // (case, the cluster the group belongs to, that group, the subnet the update names,
    // whether it is served)
    let rows: [(&str, &str, &str, &String, bool); 4] = [
        (
            "the first cluster's group to another subnet of its own VPC",
            "first-host",
            "first-nodes",
            &also_in_first,
            true,
        ),
        (
            "the first cluster's group into the second VPC",
            "first-host",
            "first-nodes",
            &also_in_second,
            false,
        ),
        (
            "the second cluster's group to another subnet of its own VPC",
            "second-host",
            "second-nodes",
            &also_in_second,
            true,
        ),
        (
            "the second cluster's group into the first VPC",
            "second-host",
            "second-nodes",
            &in_first,
            false,
        ),
    ];

    for (case, cluster, group, subnet, is_served) in rows {
        let before = stored_subnets(&router, cluster, group).await;
        let (status, body) = call(
            &router,
            "UpdateComputeNodeGroup",
            &update_group(cluster, group, Some(std::slice::from_ref(subnet))),
        )
        .await;
        let expected = if is_served {
            assert_eq!(
                status, 200,
                "case {case}: expected the update to be served, got {body}"
            );
            json!([subnet])
        } else {
            assert_eq!(
                status, 400,
                "case {case}: expected the update to be refused, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(SUBNETS_OUTSIDE_CLUSTER_VPC),
                "case {case}: got {body}"
            );
            before
        };
        assert_eq!(
            stored_subnets(&router, cluster, group).await,
            expected,
            "case {case}: expected the stored placement to be {expected}"
        );
    }
}
