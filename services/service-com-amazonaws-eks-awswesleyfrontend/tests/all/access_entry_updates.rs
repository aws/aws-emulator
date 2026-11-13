// SPDX-License-Identifier: CC0-1.0

//! `UpdateAccessEntry`, and the SHAPE of the refusal the access-entry slice
//! answers a cluster it cannot resolve with.
//!
//! Two rules are under test here, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts (the harness is the crate's
//! one copy in `tests/all/wire.rs`, and clusters come from `cluster_lifecycle`'s
//! `minimal_request`):
//!
//! * an update rewrites only the members an access entry does not fix at
//!   creation, and the store is what a read reports afterwards;
//! * a not-found refusal reports the ONE resource the operation says is missing,
//!   so an operation whose subject is the cluster builds its message FROM the
//!   request while an operation addressing the `(cluster, entry)` pair carries the
//!   slice's constant sentence.
//!
//! Every suite asserts its rule over a TABLE whose rows differ along what the rule
//! generalises over (which members a request carries, the entry `type`, the form
//! of the cluster name, which half of the pair is missing), and every assertion
//! names its case and prints the actual value.

use std::collections::BTreeSet;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, default_username, server_managed_kubernetes_groups,
    ACCESS_ENTRY_TYPES, ACCESS_ENTRY_TYPE_EC2_LINUX, ACCESS_ENTRY_TYPE_EC2_WINDOWS,
    ACCESS_ENTRY_TYPE_STANDARD, RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{access_entry_cluster_request, REGION};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The emulator's own canonical account id, which every ARN it mints carries.
const ACCOUNT: &str = "000000000000";

/// Every resource-identity member `ResourceNotFoundException` declares beside
/// `message` (`smithy_query query="shape" shape="ResourceNotFoundException"`).
const IDENTITY_MEMBERS: &[&str] = &[
    "clusterName",
    "nodegroupName",
    "fargateProfileName",
    "addonName",
    "subscriptionId",
];

fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

async fn create_cluster(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        "/clusters",
        &access_entry_cluster_request(name).to_string(),
    )
    .await
}

async fn create_entry(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/access-entries", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

fn entry_path(cluster: &str, principal: &str) -> String {
    format!(
        "/clusters/{}/access-entries/{}",
        encode_label(cluster),
        encode_label(principal)
    )
}

async fn update_entry_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    principal: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        region,
        "POST",
        &entry_path(cluster, principal),
        &body.to_string(),
    )
    .await
}

async fn update_entry(
    router: &MultiServiceRouter,
    cluster: &str,
    principal: &str,
    body: &serde_json::Value,
) -> Answer {
    update_entry_in(router, REGION, cluster, principal, body).await
}

async fn describe_entry(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    call(router, REGION, "GET", &entry_path(cluster, principal), "").await
}

/// The Kubernetes groups an answer's `accessEntry` reports, in the order returned.
fn groups(answer: &Answer) -> Vec<String> {
    answer.body["accessEntry"]["kubernetesGroups"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The epoch seconds a timestamp member of the answer's `accessEntry` carries.
fn instant(answer: &Answer, member: &str) -> f64 {
    answer.body["accessEntry"][member].as_f64().unwrap_or(-1.0)
}

/// A cluster holding one access entry of `entry_type` with `groups` and
/// `username` on it. Returns the principal ARN the entry is keyed by.
async fn cluster_with_entry(
    router: &MultiServiceRouter,
    cluster: &str,
    role: &str,
    entry_type: &str,
    kubernetes_groups: &[&str],
    username: Option<&str>,
) -> String {
    let created = create_cluster(router, cluster).await;
    assert_eq!(
        created.status, 200,
        "setup: creating cluster {cluster:?} must succeed, got {created:?}"
    );
    let principal = role_arn(role);
    let mut body = serde_json::json!({ "principalArn": principal, "type": entry_type });
    if !kubernetes_groups.is_empty() {
        body["kubernetesGroups"] = serde_json::json!(kubernetes_groups);
    }
    if let Some(username) = username {
        body["username"] = serde_json::json!(username);
    }
    let entry = create_entry(router, cluster, &body).await;
    assert_eq!(
        entry.status, 200,
        "setup: creating the {entry_type} access entry for {principal:?} must succeed, \
         got {entry:?}"
    );
    principal
}

// ---------------------------------------------------------------------------
// RULE: `UpdateAccessEntry` rewrites ONLY the members an access entry does not fix
// at creation — a member the request carries replaces the stored one, a member it
// omits leaves it alone — and the entry reads back through `DescribeAccessEntry`
// exactly as the update answered.
// ---------------------------------------------------------------------------

/// The rows differ along WHICH optional members the request carries and along the
/// size of the group list it carries (none, empty, one, several), which is what
/// the rule generalises over.
#[tokio::test]
async fn an_update_rewrites_only_the_members_the_request_carries() {
    let router = emulator();

    // (case, request body, expected groups, expected username)
    let cases: Vec<(&str, serde_json::Value, Vec<&str>, String)> = vec![
        (
            "both mutable members replaced",
            serde_json::json!({ "kubernetesGroups": ["ops"], "username": "ops-user" }),
            vec!["ops"],
            "ops-user".to_string(),
        ),
        (
            "groups only, so the username stands",
            serde_json::json!({ "kubernetesGroups": ["alpha", "beta", "gamma"] }),
            vec!["alpha", "beta", "gamma"],
            "set-at-create".to_string(),
        ),
        (
            "username only, so the groups stand",
            serde_json::json!({ "username": "renamed" }),
            vec!["view", "edit"],
            "renamed".to_string(),
        ),
        (
            "an empty group list is a value, so it clears the groups",
            serde_json::json!({ "kubernetesGroups": [] }),
            Vec::new(),
            "set-at-create".to_string(),
        ),
        (
            "a body carrying neither member changes neither",
            serde_json::json!({}),
            vec!["view", "edit"],
            "set-at-create".to_string(),
        ),
        (
            "a blank username names nobody, so Amazon EKS specifies it",
            serde_json::json!({ "username": "   " }),
            vec!["view", "edit"],
            // The username EKS picks for a STANDARD entry, from the same helper the
            // create path uses.
            default_username(ACCESS_ENTRY_TYPE_STANDARD, &role_arn("case-5")),
        ),
        (
            "more groups than the entry started with",
            serde_json::json!({ "kubernetesGroups": ["g1", "g2", "g3", "g4", "g5"] }),
            vec!["g1", "g2", "g3", "g4", "g5"],
            "set-at-create".to_string(),
        ),
        (
            "an idempotency token is carried but changes nothing else",
            serde_json::json!({
                "username": "tokened",
                "clientRequestToken": "11111111-2222-3333-4444-555555555555",
            }),
            vec!["view", "edit"],
            "tokened".to_string(),
        ),
    ];

    for (index, (case, body, expected_groups, expected_username)) in cases.iter().enumerate() {
        let cluster = format!("upd-{index}");
        let principal = cluster_with_entry(
            &router,
            &cluster,
            &format!("case-{index}"),
            ACCESS_ENTRY_TYPE_STANDARD,
            &["view", "edit"],
            Some("set-at-create"),
        )
        .await;
        let before = describe_entry(&router, &cluster, &principal).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the entry must read back before the update, got {before:?}"
        );

        let updated = update_entry(&router, &cluster, &principal, body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected the update to succeed, got {updated:?}"
        );
        assert_eq!(
            groups(&updated),
            *expected_groups,
            "case {case:?}: expected kubernetesGroups {expected_groups:?}, got {:?}",
            groups(&updated)
        );
        assert_eq!(
            updated.body["accessEntry"]["username"].as_str(),
            Some(expected_username.as_str()),
            "case {case:?}: expected username {expected_username:?}, got {:?}",
            updated.body["accessEntry"]["username"]
        );

        // The members an access entry fixes at creation are not updatable, so they
        // read back as they were written.
        for member in ["clusterName", "principalArn", "type", "accessEntryArn"] {
            assert_eq!(
                updated.body["accessEntry"][member], before.body["accessEntry"][member],
                "case {case:?}: {member} must not change across an update, got {:?}",
                updated.body["accessEntry"][member]
            );
        }
        assert_eq!(
            instant(&updated, "createdAt"),
            instant(&before, "createdAt"),
            "case {case:?}: createdAt must not move, got {:?}",
            updated.body["accessEntry"]["createdAt"]
        );
        assert!(
            instant(&updated, "modifiedAt") >= instant(&before, "modifiedAt"),
            "case {case:?}: modifiedAt must not go backwards, got {:?} after {:?}",
            updated.body["accessEntry"]["modifiedAt"],
            before.body["accessEntry"]["modifiedAt"]
        );

        // The update WROTE: a read reports exactly what the update answered, so the
        // answer came from the store rather than from the request.
        let read_back = describe_entry(&router, &cluster, &principal).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: the updated entry must read back, got {read_back:?}"
        );
        assert_eq!(
            read_back.body["accessEntry"], updated.body["accessEntry"],
            "case {case:?}: the stored entry must equal the one the update answered, \
             got {read_back:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: updates compose — each one is applied to what the previous one stored, so
// the entry's state is the fold of the calls rather than of the last body alone.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn successive_updates_compose_on_the_stored_entry() {
    let router = emulator();
    let cluster = "upd-compose";
    let principal = cluster_with_entry(
        &router,
        cluster,
        "compose",
        ACCESS_ENTRY_TYPE_STANDARD,
        &["first"],
        Some("first-user"),
    )
    .await;

    // (case, body, expected groups, expected username) applied IN ORDER to one entry.
    let steps: Vec<(&str, serde_json::Value, Vec<&str>, &str)> = vec![
        (
            "rename only",
            serde_json::json!({ "username": "second-user" }),
            vec!["first"],
            "second-user",
        ),
        (
            "regroup only, keeping the rename",
            serde_json::json!({ "kubernetesGroups": ["second", "third"] }),
            vec!["second", "third"],
            "second-user",
        ),
        (
            "clear the groups, keeping the rename",
            serde_json::json!({ "kubernetesGroups": [] }),
            vec![],
            "second-user",
        ),
        (
            "both again from the cleared state",
            serde_json::json!({ "kubernetesGroups": ["final"], "username": "final-user" }),
            vec!["final"],
            "final-user",
        ),
    ];

    for (case, body, expected_groups, expected_username) in &steps {
        let updated = update_entry(&router, cluster, &principal, body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected the update to succeed, got {updated:?}"
        );
        let read_back = describe_entry(&router, cluster, &principal).await;
        assert_eq!(
            groups(&read_back),
            *expected_groups,
            "case {case:?}: expected the stored kubernetesGroups {expected_groups:?}, \
             got {:?}",
            groups(&read_back)
        );
        assert_eq!(
            read_back.body["accessEntry"]["username"].as_str(),
            Some(*expected_username),
            "case {case:?}: expected the stored username {expected_username:?}, got {:?}",
            read_back.body["accessEntry"]["username"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a self-managed Amazon EC2 node access entry (`EC2_LINUX`, `EC2_WINDOWS`)
// carries no Kubernetes groups, so SETTING them is refused with
// `InvalidParameterException` (400) whichever call tries — the rule is about the
// entry's stored type, which an update cannot change.
// ---------------------------------------------------------------------------

/// The rows walk EVERY access-entry type the service accepts, so the refusal is
/// asserted against the whole catalogue rather than only the two types that carry
/// it.
#[tokio::test]
async fn a_self_managed_node_entry_takes_no_kubernetes_groups_on_update() {
    let router = emulator();

    for (index, entry_type) in ACCESS_ENTRY_TYPES.iter().enumerate() {
        let cluster = format!("node-upd-{index}");
        let refused = *entry_type == ACCESS_ENTRY_TYPE_EC2_LINUX
            || *entry_type == ACCESS_ENTRY_TYPE_EC2_WINDOWS;
        let principal = cluster_with_entry(
            &router,
            &cluster,
            &format!("node-{index}"),
            entry_type,
            &[],
            None,
        )
        .await;

        let answer = update_entry(
            &router,
            &cluster,
            &principal,
            &serde_json::json!({ "kubernetesGroups": ["ops"] }),
        )
        .await;
        if refused {
            assert_eq!(
                answer.status, 400,
                "case {entry_type:?}: expected 400, got {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "InvalidParameterException",
                "case {entry_type:?}: expected InvalidParameterException, got {answer:?}"
            );
            // Nothing is stored by a refused update: the entry keeps the groups it
            // had, which for this type are the ones Amazon EKS derived for it.
            let read_back = describe_entry(&router, &cluster, &principal).await;
            assert_eq!(
                groups(&read_back),
                server_managed_kubernetes_groups(entry_type)
                    .iter()
                    .map(|group| (*group).to_string())
                    .collect::<Vec<String>>(),
                "case {entry_type:?}: a refused update must store nothing, got {read_back:?}"
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {entry_type:?}: this type takes Kubernetes groups, got {answer:?}"
            );
            assert_eq!(
                groups(&answer),
                vec!["ops".to_string()],
                "case {entry_type:?}: expected the groups to be stored, got {answer:?}"
            );
        }

        // An EMPTY list sets no group, so it is accepted for every type.
        let cleared = update_entry(
            &router,
            &cluster,
            &principal,
            &serde_json::json!({ "kubernetesGroups": [] }),
        )
        .await;
        assert_eq!(
            cleared.status, 200,
            "case {entry_type:?}: setting no group must be accepted, got {cleared:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a not-found refusal reports the ONE resource the operation says is
// missing. An operation whose SUBJECT is the cluster (it only validates the
// cluster before creating in it or enumerating it) builds its message FROM the
// request — it names the cluster that could not be resolved, and sets
// `clusterName` — while an operation addressing the `(cluster, entry)` pair cannot
// tell the two halves apart and so carries the slice's constant sentence with no
// identity member at all.
// ---------------------------------------------------------------------------

/// The rows differ along the FORM of the cluster name (never created, differing
/// only in case, the two documented `clusterName` length bounds, the right name in
/// the wrong region), which is what makes a request-built message observable: the
/// message must change with the request rather than being a constant.
#[tokio::test]
async fn a_cluster_the_slice_cannot_resolve_is_named_only_where_it_is_the_subject() {
    let router = emulator();
    let present = cluster_with_entry(
        &router,
        "aer-present",
        "aer-present-role",
        ACCESS_ENTRY_TYPE_STANDARD,
        &[],
        None,
    )
    .await;

    let longest = "c".repeat(100);
    // (case, region, cluster)
    let missing: Vec<(&str, &str, &str)> = vec![
        ("a cluster that was never created", REGION, "aer-ghost"),
        (
            "a cluster name differing only in case",
            REGION,
            "AER-PRESENT",
        ),
        (
            "a cluster name at the documented length ceiling",
            REGION,
            longest.as_str(),
        ),
        ("a one-character cluster name", REGION, "q"),
        (
            "the real cluster name, signed for another region",
            "eu-west-1",
            "aer-present",
        ),
    ];

    // The messages the cluster-subject operations produce, collected to prove the
    // sentence is DERIVED from the request rather than fixed.
    let mut built_messages: BTreeSet<String> = BTreeSet::new();

    for (case, region, cluster) in &missing {
        // Half one: the cluster is the subject, so the refusal names it.
        let listed = call(
            &router,
            region,
            "GET",
            &format!("/clusters/{}/access-entries", encode_label(cluster)),
            "",
        )
        .await;
        let created = call(
            &router,
            region,
            "POST",
            &format!("/clusters/{}/access-entries", encode_label(cluster)),
            &serde_json::json!({ "principalArn": present }).to_string(),
        )
        .await;
        for (operation, answer) in [("ListAccessEntries", &listed), ("CreateAccessEntry", &created)]
        {
            let case = format!("{case} / {operation}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404, got {answer:?}"
            );
            assert_eq!(
                error_code(answer),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(cluster_not_found_message(cluster).as_str()),
                "case {case:?}: expected the message to name the missing cluster, got {:?}",
                answer.body["message"]
            );
            assert_eq!(
                answer.body["clusterName"].as_str(),
                Some(*cluster),
                "case {case:?}: expected clusterName to name the missing cluster, got {:?}",
                answer.body["clusterName"]
            );
            built_messages.insert(
                answer.body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
        }

        // Half two: the operation addresses the `(cluster, entry)` pair, so it names
        // neither half. Fixing half one must not regress this.
        let updated = update_entry_in(
            &router,
            region,
            cluster,
            &present,
            &serde_json::json!({ "username": "nobody" }),
        )
        .await;
        let described = call(&router, region, "GET", &entry_path(cluster, &present), "").await;
        let deleted = call(
            &router,
            region,
            "DELETE",
            &entry_path(cluster, &present),
            "",
        )
        .await;
        for (operation, answer) in [
            ("UpdateAccessEntry", &updated),
            ("DescribeAccessEntry", &described),
            ("DeleteAccessEntry", &deleted),
        ] {
            let case = format!("{case} / {operation}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404, got {answer:?}"
            );
            assert_eq!(
                error_code(answer),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(RESOURCE_NOT_FOUND_MESSAGE),
                "case {case:?}: expected the slice's constant sentence, got {:?}",
                answer.body["message"]
            );
            for member in IDENTITY_MEMBERS {
                assert_eq!(
                    answer.body[member],
                    serde_json::Value::Null,
                    "case {case:?}: the payload must not carry {member}, got {:?}",
                    answer.body[member]
                );
            }
        }
    }

    // Four distinct cluster names were asked for, so four distinct sentences came
    // back: the message is built from the request, not fixed. (The fifth row names
    // `aer-present`, whose sentence the other-region row shares with nothing else.)
    let distinct_names: BTreeSet<&str> = missing.iter().map(|(_, _, cluster)| *cluster).collect();
    assert_eq!(
        built_messages.len(),
        distinct_names.len(),
        "the cluster-subject message must change with the request: {} distinct names \
         produced {} distinct sentences {built_messages:?}",
        distinct_names.len(),
        built_messages.len()
    );

    // And the refusal is about ABSENCE, not about the shape of the request: the same
    // update succeeds against the cluster that IS there.
    let allowed = update_entry(
        &router,
        "aer-present",
        &present,
        &serde_json::json!({ "username": "somebody" }),
    )
    .await;
    assert_eq!(
        allowed.status, 200,
        "the same update against the cluster that exists must succeed, got {allowed:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: an access entry is addressed through its cluster, which a cluster name
// identifies only within one region — so an update is applied to the entry of the
// cluster in the region the request is signed for, and to no other.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_reaches_only_the_region_it_is_signed_for() {
    let router = emulator();
    let cluster = "aer-shared-name";
    let principal = role_arn("shared");

    // The same cluster name and the same principal in three regions: the rows differ
    // along which region the update is signed for.
    let regions = ["us-west-2", "eu-central-1", "ap-southeast-1"];
    for region in regions {
        let created = call(
            &router,
            region,
            "POST",
            "/clusters",
            &access_entry_cluster_request(cluster).to_string(),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "setup: creating {cluster:?} in {region} must succeed, got {created:?}"
        );
        let entry = call(
            &router,
            region,
            "POST",
            &format!("/clusters/{}/access-entries", encode_label(cluster)),
            &serde_json::json!({ "principalArn": principal, "username": region }).to_string(),
        )
        .await;
        assert_eq!(
            entry.status, 200,
            "setup: creating the entry in {region} must succeed, got {entry:?}"
        );
    }

    for region in regions {
        let updated = update_entry_in(
            &router,
            region,
            cluster,
            &principal,
            &serde_json::json!({ "username": format!("{region}-updated") }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {region:?}: expected the update to succeed, got {updated:?}"
        );
    }

    // Each region's entry carries its OWN new username: no update crossed over.
    for region in regions {
        let read_back = call(&router, region, "GET", &entry_path(cluster, &principal), "").await;
        assert_eq!(
            read_back.body["accessEntry"]["username"].as_str(),
            Some(format!("{region}-updated").as_str()),
            "case {region:?}: expected the region's own updated username, got {read_back:?}"
        );
    }
}
