// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two deletes of a resource INSIDE a cluster:
//! `DeleteNodegroup` and `DeleteFargateProfile`, driven over the wire through the
//! same `MultiServiceRouter` the production binary mounts.
//!
//! One rule, asserted over a TABLE whose rows differ along which resource is
//! deleted, which half of the addressed `(cluster, resource)` pair is missing and
//! whether the resource has settled: a delete removes exactly the resource the pair
//! names, answers with it in the status a delete leaves it in, and a miss of either
//! half is the same not-found the matching read gives.
//!
//! The removal itself is ASYNCHRONOUS — the resource stays readable in `DELETING`
//! until the clock has finished it, which `tests/all/asynchronous_child_removal.rs`
//! is about — so a row here that reads the resource as gone waits for the removal to
//! finish by LOOKING AGAIN, never for a span.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one
//! in `tests/all/wire.rs`.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    NODEGROUP_ACTIVATION_DELAY, UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::REGION;
use crate::nodegroup_lifecycle::{create_cluster, create_nodegroup_in, minimal_request};
use crate::wire::{call, emulator, encode_label, error_code, read_until, Answer};

/// A little longer than the span the clock owes a transition, so a read after it is
/// certainly past the due instant.
fn past(delay: Duration) -> Duration {
    delay + Duration::from_millis(250)
}

async fn create_fargate_profile(
    router: &MultiServiceRouter,
    cluster: &str,
    name: &str,
) -> Answer {
    let body = serde_json::json!({
        "fargateProfileName": name,
        "podExecutionRoleArn": format!(
            "arn:aws:iam::{}:role/eksFargatePodExecutionRole",
            engine::emulator_account_id()
        ),
        "selectors": [{ "namespace": "default" }],
    });
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/fargate-profiles", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn delete_nodegroup(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &format!(
            "/clusters/{}/node-groups/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

async fn describe_nodegroup(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/node-groups/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

async fn delete_fargate_profile(
    router: &MultiServiceRouter,
    cluster: &str,
    name: &str,
) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &format!(
            "/clusters/{}/fargate-profiles/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

async fn describe_fargate_profile(
    router: &MultiServiceRouter,
    cluster: &str,
    name: &str,
) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/fargate-profiles/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

/// Read the node group until it reports `ACTIVE`, through the shared
/// [`crate::wire::read_until`]: the clock-owed provisioning is waited for by looking
/// again, never by waiting out a span.
async fn nodegroup_until_active(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    name: &str,
) -> Answer {
    read_until(
        case,
        &format!("the node group {name:?} reports ACTIVE"),
        || describe_nodegroup(router, cluster, name),
        |answer| answer.status == 200 && answer.body["nodegroup"]["status"] == "ACTIVE",
    )
    .await
}

/// [`nodegroup_until_active`] for the Fargate profile.
async fn fargate_profile_until_active(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    name: &str,
) -> Answer {
    read_until(
        case,
        &format!("the profile {name:?} reports ACTIVE"),
        || describe_fargate_profile(router, cluster, name),
        |answer| answer.status == 200 && answer.body["fargateProfile"]["status"] == "ACTIVE",
    )
    .await
}

/// Read the node group until the read reports it absent: the END of the removal the
/// delete accepted, waited for the same way.
async fn nodegroup_until_gone(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    name: &str,
) -> Answer {
    read_until(
        case,
        &format!("the removal of the node group {name:?} finished"),
        || describe_nodegroup(router, cluster, name),
        |answer| answer.status == 404,
    )
    .await
}

/// [`nodegroup_until_gone`] for the Fargate profile.
async fn fargate_profile_until_gone(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    name: &str,
) -> Answer {
    read_until(
        case,
        &format!("the removal of the profile {name:?} finished"),
        || describe_fargate_profile(router, cluster, name),
        |answer| answer.status == 404,
    )
    .await
}

// ---------------------------------------------------------------------------
// RULE: a delete removes the one resource the `(cluster, resource)` pair names and
// answers with it in the status a delete leaves it in; the matching read then
// reports it gone.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_removes_exactly_the_resource_the_pair_names() {
    let router = emulator();
    create_cluster(&router, "deletions").await;

    // Two node groups and two Fargate profiles, so each delete can be shown to leave
    // its sibling alone.
    for name in ["ng-one", "ng-two"] {
        let created =
            create_nodegroup_in(&router, REGION, "deletions", &minimal_request(name)).await;
        assert_eq!(
            created.status, 200,
            "setup: expected 200 from CreateNodegroup for {name:?}, got {created:?}"
        );
    }
    for name in ["fp-one", "fp-two"] {
        let created = create_fargate_profile(&router, "deletions", name).await;
        assert_eq!(
            created.status, 200,
            "setup: expected 200 from CreateFargateProfile for {name:?}, got {created:?}"
        );
    }
    // Each child settles on the clock; that is waited for by LOOKING AGAIN rather
    // than for a span, so the deletes below act on resources that have settled.
    for name in ["ng-one", "ng-two"] {
        nodegroup_until_active(&router, "the node group delete", "deletions", name).await;
    }
    for name in ["fp-one", "fp-two"] {
        fargate_profile_until_active(&router, "the Fargate profile delete", "deletions", name)
            .await;
    }

    let deleted = delete_nodegroup(&router, "deletions", "ng-one").await;
    assert_eq!(
        deleted.status, 200,
        "case the node group delete: expected 200, got {deleted:?}"
    );
    assert_eq!(
        deleted.body["nodegroup"]["nodegroupName"],
        serde_json::json!("ng-one"),
        "case the node group delete: expected the deleted node group, got {deleted:?}"
    );
    assert_eq!(
        deleted.body["nodegroup"]["status"],
        serde_json::json!("DELETING"),
        "case the node group delete: expected status DELETING, got {:?}",
        deleted.body["nodegroup"]["status"]
    );
    let gone = nodegroup_until_gone(&router, "the node group delete", "deletions", "ng-one").await;
    assert_eq!(
        gone.status, 404,
        "case the node group delete: the resource is gone once the removal finished, got \
         {gone:?}"
    );
    let kept = describe_nodegroup(&router, "deletions", "ng-two").await;
    assert_eq!(
        kept.status, 200,
        "case the node group delete: the sibling is left alone, got {kept:?}"
    );

    let deleted = delete_fargate_profile(&router, "deletions", "fp-one").await;
    assert_eq!(
        deleted.status, 200,
        "case the Fargate profile delete: expected 200, got {deleted:?}"
    );
    assert_eq!(
        deleted.body["fargateProfile"]["status"],
        serde_json::json!("DELETING"),
        "case the Fargate profile delete: expected status DELETING, got {:?}",
        deleted.body["fargateProfile"]["status"]
    );
    let gone = fargate_profile_until_gone(
        &router,
        "the Fargate profile delete",
        "deletions",
        "fp-one",
    )
    .await;
    assert_eq!(
        gone.status, 404,
        "case the Fargate profile delete: the resource is gone once the removal finished, got \
         {gone:?}"
    );
    let kept = describe_fargate_profile(&router, "deletions", "fp-two").await;
    assert_eq!(
        kept.status, 200,
        "case the Fargate profile delete: the sibling is left alone, got {kept:?}"
    );
}

/// The table spans which half of the addressed pair is missing, for each of the two
/// deletes — and a second delete of the resource the first already removed, which is
/// the same miss.
#[tokio::test]
async fn a_delete_of_a_resource_the_pair_does_not_name_is_not_found() {
    let router = emulator();
    create_cluster(&router, "delete-misses").await;
    let created =
        create_nodegroup_in(&router, REGION, "delete-misses", &minimal_request("ng")).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateNodegroup, got {created:?}"
    );
    let created = create_fargate_profile(&router, "delete-misses", "fp").await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateFargateProfile, got {created:?}"
    );
    nodegroup_until_active(&router, "the node group misses", "delete-misses", "ng").await;
    fargate_profile_until_active(&router, "the profile misses", "delete-misses", "fp").await;

    for (case, cluster, name, nodegroup) in [
        ("a node group whose cluster was never created", "no-such-cluster", "ng", true),
        ("a node group the cluster does not carry", "delete-misses", "no-such-ng", true),
        ("a profile whose cluster was never created", "no-such-cluster", "fp", false),
        ("a profile the cluster does not carry", "delete-misses", "no-such-fp", false),
    ] {
        let answer = if nodegroup {
            delete_nodegroup(&router, cluster, name).await
        } else {
            delete_fargate_profile(&router, cluster, name).await
        };
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code:?}"
        );
    }

    // A second delete of the same resource, once the removal the first accepted has
    // finished, is the same miss.
    for (case, nodegroup, name) in [
        ("the node group, twice", true, "ng"),
        ("the Fargate profile, twice", false, "fp"),
    ] {
        let first = if nodegroup {
            delete_nodegroup(&router, "delete-misses", name).await
        } else {
            delete_fargate_profile(&router, "delete-misses", name).await
        };
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first delete succeeds, got {first:?}"
        );
        if nodegroup {
            nodegroup_until_gone(&router, case, "delete-misses", name).await;
        } else {
            fargate_profile_until_gone(&router, case, "delete-misses", name).await;
        }
        let second = if nodegroup {
            delete_nodegroup(&router, "delete-misses", name).await
        } else {
            delete_fargate_profile(&router, "delete-misses", name).await
        };
        assert_eq!(
            second.status, 404,
            "case {case:?}: expected HTTP 404 from the second delete, got {second:?}"
        );
    }
}

/// A node group taking an update is in use: the change that update carries is still
/// owed to it, so it cannot be removed from under it — and once the update
/// completes, the delete goes through.
#[tokio::test]
async fn a_node_group_taking_an_update_is_in_use() {
    let router = emulator();
    create_cluster(&router, "delete-in-use").await;
    let created =
        create_nodegroup_in(&router, REGION, "delete-in-use", &minimal_request("ng")).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateNodegroup, got {created:?}"
    );
    tokio::time::sleep(past(NODEGROUP_ACTIVATION_DELAY)).await;

    let started = call(
        &router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/node-groups/ng/update-config",
            encode_label("delete-in-use")
        ),
        &serde_json::json!({ "scalingConfig": { "desiredSize": 1 } }).to_string(),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "setup: expected 200 from UpdateNodegroupConfig, got {started:?}"
    );

    let refused = delete_nodegroup(&router, "delete-in-use", "ng").await;
    assert_eq!(
        refused.status, 409,
        "case while an update runs: expected HTTP 409, got {refused:?}"
    );
    let code = error_code(&refused);
    assert_eq!(
        code, "ResourceInUseException",
        "case while an update runs: expected ResourceInUseException, got {code:?}"
    );

    tokio::time::sleep(past(UPDATE_COMPLETION_DELAY)).await;
    let deleted = delete_nodegroup(&router, "delete-in-use", "ng").await;
    assert_eq!(
        deleted.status, 200,
        "case once the update completes: expected 200, got {deleted:?}"
    );
}
