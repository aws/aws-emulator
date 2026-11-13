// SPDX-License-Identifier: CC0-1.0

//! The Kubernetes groups an access entry reports, and who decides them.
//!
//! RULE: an access entry of a NODE type stands for a node (or a Fargate Pod) that
//! Amazon EKS admits to the cluster rather than for a principal the caller maps
//! itself, so EKS specifies what that entry authenticates as: the `system:node:`
//! username and the group Kubernetes's node authorizer binds — which the created
//! entry reports even though the request sends no `kubernetesGroups` at all. A
//! `STANDARD` entry authenticates as the IAM principal itself, and its groups are
//! entirely the caller's. That is also why the self-managed Amazon EC2 types refuse
//! a `kubernetesGroups` of the caller's: "if you set the value to `EC2_LINUX` or
//! `EC2_WINDOWS`, you can't specify values for `kubernetesGroups`"
//! (`CreateAccessEntryRequest$type`) — the grant is the server's to confer.
//!
//! The table spans every `type` the service accepts
//! (`crate::state::ACCESS_ENTRY_TYPES`, the set that member documents) rather than
//! the one type the issue cited, and each row is checked on the create's own answer
//! AND on the entry read back, because what the entry reports has to come from the
//! stored record. Two further rows span what the rule generalises over: a request
//! that DOES send groups for a type that takes them keeps exactly those, and an
//! update that sends an empty list leaves a derived grant where it is.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    is_node_access_entry_type, server_managed_kubernetes_groups, ACCESS_ENTRY_TYPES,
    ACCESS_ENTRY_TYPE_STANDARD, NODE_AUTHORIZATION_GROUP,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::access_entry_cluster_request;
use crate::cluster_lifecycle::REGION;
use crate::wire::{call, emulator, encode_label, Answer};

/// The cluster every entry here is created on.
const CLUSTER: &str = "node-groups-host";

async fn create_cluster(router: &MultiServiceRouter) {
    let created = call(
        router,
        REGION,
        "POST",
        "/clusters",
        &access_entry_cluster_request(CLUSTER).to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: creating the cluster must succeed, got {created:?}"
    );
}

async fn create_entry(router: &MultiServiceRouter, body: &serde_json::Value) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/access-entries", encode_label(CLUSTER)),
        &body.to_string(),
    )
    .await
}

async fn describe_entry(router: &MultiServiceRouter, principal: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/access-entries/{}",
            encode_label(CLUSTER),
            encode_label(principal)
        ),
        "",
    )
    .await
}

/// The `kubernetesGroups` an answer reports, as a list of strings.
fn groups(answer: &Answer) -> Vec<String> {
    answer.body["accessEntry"]["kubernetesGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// RULE: the groups of a node-type entry are Amazon EKS's to derive; a STANDARD
// entry's are the caller's.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_entry_reports_the_groups_its_type_derives_when_the_request_sends_none() {
    let router = emulator();
    create_cluster(&router).await;

    for (index, entry_type) in ACCESS_ENTRY_TYPES.iter().enumerate() {
        let principal = format!("arn:aws:iam::000000000000:role/derived-{index}");
        let created = create_entry(
            &router,
            &serde_json::json!({ "principalArn": principal, "type": entry_type }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {entry_type:?}: the entry must be created, got {created:?}"
        );

        // The derived set is the service's own answer to "what does this type
        // confer", read from the one definition of it rather than written out per
        // row: a node type carries the node-authorization group, `STANDARD` carries
        // nothing.
        let expected: Vec<String> = server_managed_kubernetes_groups(entry_type)
            .iter()
            .map(|group| (*group).to_string())
            .collect();
        assert_eq!(
            groups(&created),
            expected,
            "case {entry_type:?}: the create must report the groups its type derives, got \
             {created:?}"
        );
        let read = describe_entry(&router, &principal).await;
        assert_eq!(
            groups(&read),
            expected,
            "case {entry_type:?}: the stored entry must carry them too, got {read:?}"
        );

        // The two halves of the rule, stated against the type: a type Amazon EKS
        // admits a NODE through (`NODE_ACCESS_ENTRY_TYPES`, the list
        // `CreateAccessEntryRequest$principalArn` publishes) confers the node
        // authorizer's group; every other type's groups are the caller's.
        if is_node_access_entry_type(entry_type) {
            assert_eq!(
                expected,
                vec![NODE_AUTHORIZATION_GROUP.to_string()],
                "case {entry_type:?}: a node entry carries the node-authorization group, got \
                 {expected:?}"
            );
        } else {
            assert!(
                expected.is_empty(),
                "case {entry_type:?}: this type's groups are the caller's, got {expected:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CONTROL: a request that sends groups for a type that takes them keeps exactly
// those — the derivation fills a gap rather than overriding the caller.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn groups_the_request_sends_are_kept_where_the_type_accepts_them() {
    let router = emulator();
    create_cluster(&router).await;

    // The rows differ along the type (one that takes groups and stands for a node,
    // and `STANDARD`) and along how many groups the request sends.
    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "a STANDARD entry with one group",
            ACCESS_ENTRY_TYPE_STANDARD,
            vec!["view-only"],
        ),
        (
            "a STANDARD entry with several groups",
            ACCESS_ENTRY_TYPE_STANDARD,
            vec!["ops", "readers"],
        ),
        (
            "a Fargate entry with its own group",
            "FARGATE_LINUX",
            vec!["system:nodes"],
        ),
    ];

    for (index, (case, entry_type, sent)) in rows.iter().enumerate() {
        let principal = format!("arn:aws:iam::000000000000:role/sent-{index}");
        let created = create_entry(
            &router,
            &serde_json::json!({
                "principalArn": principal,
                "type": entry_type,
                "kubernetesGroups": sent,
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the entry must be created, got {created:?}"
        );
        let expected: Vec<String> = sent.iter().map(|group| (*group).to_string()).collect();
        assert_eq!(
            groups(&created),
            expected,
            "case {case:?}: the request's own groups must be kept, got {created:?}"
        );
        let read = describe_entry(&router, &principal).await;
        assert_eq!(
            groups(&read),
            expected,
            "case {case:?}: the stored entry must carry them, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// CONTROL: an update cannot drop a derived grant by sending an empty list — those
// groups are the server's, so there is nothing for the caller to clear.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_with_no_groups_leaves_a_derived_grant_where_it_is() {
    let router = emulator();
    create_cluster(&router).await;

    // The rows are the node types, which are the types a grant is derived for.
    for (index, entry_type) in ACCESS_ENTRY_TYPES
        .iter()
        .filter(|entry_type| !server_managed_kubernetes_groups(entry_type).is_empty())
        .enumerate()
    {
        let principal = format!("arn:aws:iam::000000000000:role/cleared-{index}");
        let created = create_entry(
            &router,
            &serde_json::json!({ "principalArn": principal, "type": entry_type }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {entry_type:?}: the entry must be created, got {created:?}"
        );
        let updated = call(
            &router,
            REGION,
            "POST",
            &format!(
                "/clusters/{}/access-entries/{}",
                encode_label(CLUSTER),
                encode_label(&principal)
            ),
            &serde_json::json!({ "kubernetesGroups": [] }).to_string(),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {entry_type:?}: an empty list is not a refusal, got {updated:?}"
        );
        let expected: Vec<String> = server_managed_kubernetes_groups(entry_type)
            .iter()
            .map(|group| (*group).to_string())
            .collect();
        assert_eq!(
            groups(&updated),
            expected,
            "case {entry_type:?}: the derived grant must stand, got {updated:?}"
        );
        let read = describe_entry(&router, &principal).await;
        assert_eq!(
            groups(&read),
            expected,
            "case {entry_type:?}: the stored entry must still carry it, got {read:?}"
        );
    }
}
