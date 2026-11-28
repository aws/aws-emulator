// SPDX-License-Identifier: CC0-1.0

//! `DeleteCapacityProvider` never RESOLVES the optional `cluster` member, driven end to end
//! over the shared wire harness in [`crate::ecs_wire`] with `DescribeCapacityProviders` and
//! `DescribeClusters` as the readers.
//!
//! The rule: the subject of this operation is the capacity provider its `capacityProvider`
//! member names — the operation's one required member and its `resourceIdentifier`
//! (`CapacityProviderName`; `smithy_query` query="operation"
//! operation="DeleteCapacityProvider") — so WHAT the call answers, the error class of a
//! refusal included, is decided by that provider alone, and the optional `cluster`, which
//! only says where a cluster-scoped provider lives, is resolved against nothing: a value
//! that names no live cluster of the account can neither refuse a deletable provider nor
//! turn the refusal a non-deletable one earns into a cluster refusal.
//!
//! What the sibling suites already pin, and this one does not repeat: that a deletable
//! provider is retired whatever the `cluster` names
//! (`capacity_provider_delete_cluster_member`), that the capacity-provider operations whose
//! SUBJECT is a cluster still refuse one that does not resolve (the same suite), and the
//! refusal classes themselves (`capacity_provider_deletion`). The dimension here is the
//! CROSS of the two: every outcome of a delete — each refusal class the operation reserves,
//! and the association that is reported on the provider rather than raised — asked with the
//! `cluster` member carrying nothing, a live cluster, a name no cluster of the account
//! answers to, that name in its ARN form, and a cluster the account holds only as a
//! tombstone.
//!
//! Nothing here reaches into the state struct: every value asserted is read back through a
//! describe.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::capacity_providers::{group_arn, managed_instances};
use crate::ecs_wire::{
    associate_capacity_providers, create_auto_scaling_group, create_cluster,
    describe_cluster_attachments, list, ok, poll_until, refusal, service, string, ACCOUNT,
    SIGNED_REGION,
};

/// A cluster name the account holds no record of at all.
const GHOST_CLUSTER: &str = "nosuchcluster";

/// The whole of the sentence a delete of one of the providers Amazon ECS RESERVES earns.
const RESERVED_REFUSAL: &str =
    "The specified capacity provider is reserved and cannot be modified by this operation. \
     Specify a valid name or ARN and try again.";

/// The sentence a `capacityProvider` that resolves to no provider of this account earns,
/// in the class Amazon ECS answers an identifier it cannot RESOLVE in.
const UNRESOLVED_PROVIDER_REFUSAL: &str = "Failed to get info for provided CapacityProvider";

/// The sentence an identifier that is an ARN of another account earns.
const ACCOUNT_MISMATCH_REFUSAL: &str = "AccountIDs mismatch";

/// The ARN form of a capacity provider of this account, which `capacityProvider` accepts
/// beside the short name.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:capacity-provider/{name}")
}

/// The ARN form of a cluster of this account, which `cluster` accepts beside the short name.
fn cluster_arn(name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{name}")
}

/// The `(status, updateStatus)` pair a reported provider carries — the pair every answered
/// delete is read off.
fn lifecycle(provider: &Value, case: &str) -> (String, Option<String>) {
    (
        string(provider, "status", case),
        provider["updateStatus"].as_str().map(str::to_string),
    )
}

/// Describe one capacity provider by name and answer it.
async fn describe_provider(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeCapacityProviders",
        json!({ "capacityProviders": [name] }),
        case,
    )
    .await;
    let providers = list(&body, "capacityProviders", case);
    assert_eq!(
        providers.len(),
        1,
        "case {case:?}: expected exactly one described provider, got {body}"
    );
    providers[0].clone()
}

/// Create one capacity provider over an Auto Scaling group of the caller's, created first
/// through that service's own API.
async fn create_provider(router: &MultiServiceRouter, name: &str, group: &str, case: &str) {
    create_auto_scaling_group(router, group, case).await;
    ok(
        router,
        "CreateCapacityProvider",
        json!({
            "name": name,
            "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn(group) },
        }),
        case,
    )
    .await;
}

/// What the `cluster` member of a delete carries in one row, beside how that row reads.
///
/// The variants span what the member can be given short of a value this operation would
/// have to resolve: nothing, a cluster the account does hold, a name no cluster answers to
/// in either form it may be written in, and a cluster the account holds only as a
/// tombstone.
fn cluster_values() -> [(&'static str, Option<String>); 5] {
    [
        ("with no cluster named", None),
        ("through a live cluster", Some("live".to_string())),
        (
            "through a cluster nothing created",
            Some(GHOST_CLUSTER.to_string()),
        ),
        (
            "through that cluster in its ARN form",
            Some(cluster_arn(GHOST_CLUSTER)),
        ),
        ("through a deleted cluster", Some("deleted".to_string())),
    ]
}

/// Create the clusters [`cluster_values`] names: one live, and one left as a tombstone.
async fn create_named_clusters(router: &MultiServiceRouter) {
    create_cluster(router, "live", "setup").await;
    create_cluster(router, "deleted", "setup").await;
    ok(
        router,
        "DeleteCluster",
        json!({ "cluster": "deleted" }),
        "setup",
    )
    .await;
}

#[tokio::test]
async fn the_refusal_class_of_a_delete_is_decided_by_the_provider_the_request_names() {
    let router = service();
    create_named_clusters(&router).await;
    create_provider(&router, "deletable", "asg-deletable", "setup").await;

    // The provider rows span the refusals this operation reserves: the two providers Amazon
    // ECS manages for every account and cannot be asked to retire, a `capacityProvider` that
    // resolves to nothing in both forms the member accepts, and an identifier of another
    // account. Each is asked once per `cluster` value, and the pair it earns may not move.
    let providers: [(&str, Value, &str, &str); 5] = [
        (
            "the FARGATE provider Amazon ECS reserves",
            json!("FARGATE"),
            "ClientException",
            RESERVED_REFUSAL,
        ),
        (
            "the FARGATE_SPOT provider Amazon ECS reserves",
            json!("FARGATE_SPOT"),
            "ClientException",
            RESERVED_REFUSAL,
        ),
        (
            "a short name no provider of the account answers to",
            json!("ghostprovider"),
            "ClientException",
            UNRESOLVED_PROVIDER_REFUSAL,
        ),
        (
            "that name in the ARN form the member accepts",
            json!(provider_arn("ghostprovider")),
            "ClientException",
            UNRESOLVED_PROVIDER_REFUSAL,
        ),
        (
            "a provider ARN of another account",
            json!(format!(
                "arn:aws:ecs:{SIGNED_REGION}:999999999999:capacity-provider/deletable"
            )),
            "InvalidParameterException",
            ACCOUNT_MISMATCH_REFUSAL,
        ),
    ];

    for (provider_case, reference, code, message) in &providers {
        for (cluster_case, cluster) in cluster_values() {
            let case = format!("{provider_case}, {cluster_case}");
            let mut request = json!({ "capacityProvider": reference });
            if let Some(cluster) = cluster {
                request["cluster"] = json!(cluster);
            }
            let (answered_code, answered_message) =
                refusal(&router, "DeleteCapacityProvider", request, &case).await;
            assert_eq!(
                (answered_code.as_str(), answered_message.as_str()),
                (*code, *message),
                "case {case:?}: expected the refusal the provider earns, got \
                 {answered_code:?} / {answered_message:?}"
            );
        }
    }

    // The positive control: the SAME resolution path, asked through the cluster nothing
    // created, retires the one provider this account does hold — so the refusals above are
    // the provider being resolved and not the `cluster` being refused.
    let case = "the stored provider deleted through a cluster nothing created";
    let answer = ok(
        &router,
        "DeleteCapacityProvider",
        json!({ "capacityProvider": "deletable", "cluster": GHOST_CLUSTER }),
        case,
    )
    .await;
    let deleted = answer["capacityProvider"].clone();
    assert_eq!(
        lifecycle(&deleted, case),
        (
            "INACTIVE".to_string(),
            Some("DELETE_IN_PROGRESS".to_string())
        ),
        "case {case:?}: expected the provider retired with its release in flight, got \
         {deleted}"
    );
    let read = describe_provider(&router, "deletable", case).await;
    assert_eq!(
        lifecycle(&read, case),
        lifecycle(&deleted, case),
        "case {case:?}: the describe must report what the delete stored, got {read}"
    );

    // The same control over the provider the model's own documentation scopes to a cluster
    // (`DeleteCapacityProvider$cluster`: "Managed instances capacity providers are
    // cluster-scoped and can only be deleted from their associated cluster"): the recorded
    // behaviour is that the delete resolves it by name all the same, so the cluster-scoped
    // provider is retired through a cluster nothing created too. Its create is a transition
    // the CLOCK owes, so the delete is asked once the create has SETTLED — the condition,
    // not a span.
    let case = "a cluster-scoped provider deleted through a cluster nothing created";
    ok(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "scopeddeletable",
            "cluster": "live",
            "managedInstancesProvider": managed_instances(),
        }),
        case,
    )
    .await;
    poll_until(
        case,
        "the create of the cluster-scoped provider to settle",
        || describe_provider(&router, "scopeddeletable", case),
        |reported| reported["status"] == json!("ACTIVE"),
    )
    .await;
    let answer = ok(
        &router,
        "DeleteCapacityProvider",
        json!({ "capacityProvider": "scopeddeletable", "cluster": GHOST_CLUSTER }),
        case,
    )
    .await;
    let scoped = answer["capacityProvider"].clone();
    assert_eq!(
        lifecycle(&scoped, case),
        (
            "INACTIVE".to_string(),
            Some("DELETE_IN_PROGRESS".to_string())
        ),
        "case {case:?}: expected the cluster-scoped provider retired with its release in \
         flight, got {scoped}"
    );
    let read = describe_provider(&router, "scopeddeletable", case).await;
    assert_eq!(
        lifecycle(&read, case),
        lifecycle(&scoped, case),
        "case {case:?}: the describe must report what the delete stored, got {read}"
    );
}

#[tokio::test]
async fn an_association_is_reported_on_the_provider_whatever_cluster_the_delete_names() {
    // The other outcome of this operation: a provider a live cluster still lists is not
    // deleted and the call is ANSWERED on the provider all the same. That outcome is the
    // provider's too, so an unresolvable `cluster` does not pre-empt it either — and once
    // the association is gone the identical request, through the identical unresolvable
    // cluster, retires the provider.
    let router = service();
    create_named_clusters(&router).await;
    create_cluster(&router, "holder", "setup").await;
    create_provider(&router, "held", "asg-held", "setup").await;
    associate_capacity_providers(&router, "holder", &["held"], "setup").await;

    for (cluster_case, cluster) in cluster_values() {
        let case = format!("a provider a live cluster lists, {cluster_case}");
        let mut request = json!({ "capacityProvider": "held" });
        if let Some(cluster) = cluster {
            request["cluster"] = json!(cluster);
        }
        let answer = ok(&router, "DeleteCapacityProvider", request, &case).await;
        let held = answer["capacityProvider"].clone();
        assert_eq!(
            lifecycle(&held, &case),
            ("ACTIVE".to_string(), Some("DELETE_FAILED".to_string())),
            "case {case:?}: expected the provider left ACTIVE with its delete failed, got \
             {held}"
        );
        let read = describe_provider(&router, "held", &case).await;
        assert_eq!(
            lifecycle(&read, &case),
            lifecycle(&held, &case),
            "case {case:?}: the describe must report what the delete stored, got {read}"
        );
        let holder = describe_cluster_attachments(&router, "holder", &case).await;
        assert!(
            list(&holder, "capacityProviders", &case).contains(&json!("held")),
            "case {case:?}: the cluster that holds the provider still lists it, got {holder}"
        );
    }

    // The association removed, the same delete through the same cluster nothing created is
    // the one that retires the provider.
    associate_capacity_providers(&router, "holder", &[], "no longer held").await;
    let case = "the association removed, deleted through a cluster nothing created";
    let answer = ok(
        &router,
        "DeleteCapacityProvider",
        json!({ "capacityProvider": "held", "cluster": cluster_arn(GHOST_CLUSTER) }),
        case,
    )
    .await;
    let retired = answer["capacityProvider"].clone();
    assert_eq!(
        lifecycle(&retired, case),
        (
            "INACTIVE".to_string(),
            Some("DELETE_IN_PROGRESS".to_string())
        ),
        "case {case:?}: expected the provider retired with its release in flight, got \
         {retired}"
    );
    let read = describe_provider(&router, "held", case).await;
    assert_eq!(
        lifecycle(&read, case),
        lifecycle(&retired, case),
        "case {case:?}: the describe must report what the delete stored, got {read}"
    );
}
