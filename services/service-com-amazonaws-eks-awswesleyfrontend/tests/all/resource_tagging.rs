// SPDX-License-Identifier: CC0-1.0

//! WHICH resources the tag plane can address.
//!
//! RULE: `TagResource`, `UntagResource` and `ListTagsForResource` take "the Amazon
//! Resource Name (ARN) of the resource" rather than a cluster, so every resource
//! this service mints an ARN for is addressable by that ARN: the tags land on that
//! resource's OWN record, so they are the tags its own read reports and the tags the
//! listing answers. A well-formed ARN of this service that no stored resource
//! carries stays the `NotFoundException` (404) the absent case has always been, and a
//! value that is not a resource ARN of this service at all is the
//! `BadRequestException` (400) it has always been.
//!
//! The table spans the RESOURCE KINDS this service holds — a cluster and each child
//! resource whose create answers with an ARN — because the rule is about the ARN a
//! resource carries rather than about any one kind; the issue was filed on an EKS Pod
//! Identity association, which is one row of it. Each row is driven through the whole
//! plane: the create-time tags are listed, a tag call merges into them, the resource's
//! own describe reports the merge, and an untag removes exactly the key it names.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    INVALID_RESOURCE_ARN_MESSAGE, TAG_RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{access_entry_cluster_request, REGION};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The cluster every child resource below hangs off.
const CLUSTER: &str = "tagged-resources";

/// A role ARN of the account this emulator serves.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::000000000000:role/{name}")
}

/// The create-time tags every resource below is created with, so the merge a tag
/// call performs is observable as a MERGE rather than a replacement.
fn created_with() -> serde_json::Value {
    serde_json::json!({ "owner": "platform" })
}

async fn post(router: &MultiServiceRouter, uri: &str, body: &serde_json::Value) -> Answer {
    call(router, REGION, "POST", uri, &body.to_string()).await
}

async fn get(router: &MultiServiceRouter, uri: &str) -> Answer {
    call(router, REGION, "GET", uri, "").await
}

/// One row: a resource kind, the ARN its create answered with, and the path its own
/// describe reads it back at.
struct Resource {
    case: &'static str,
    arn: String,
    describe_uri: String,
    /// The member of the describe's body that carries the resource, so the row can
    /// assert the tags on the resource itself.
    payload: &'static str,
}

/// Create the cluster and one of every child resource this service mints an ARN for,
/// and report what each create answered with.
async fn fixtures(router: &MultiServiceRouter) -> Vec<Resource> {
    let cluster_path = encode_label(CLUSTER);
    let mut body = access_entry_cluster_request(CLUSTER);
    body["tags"] = created_with();
    let created = post(router, "/clusters", &body).await;
    assert_eq!(
        created.status, 200,
        "setup: creating the cluster must succeed, got {created:?}"
    );

    let mut rows = vec![Resource {
        case: "a cluster",
        arn: created.cluster()["arn"]
            .as_str()
            .expect("the cluster's ARN")
            .to_string(),
        describe_uri: format!("/clusters/{cluster_path}"),
        payload: "cluster",
    }];

    // (case, the create's path, its body, the member carrying the ARN, the describe's
    // path, the member carrying the resource). Every create names `tags`, so each
    // resource starts with the same map.
    let children: Vec<(&'static str, String, serde_json::Value, &str, String, &'static str)> = vec![
        (
            "a managed node group",
            format!("/clusters/{cluster_path}/node-groups"),
            serde_json::json!({
                "nodegroupName": "tagged-nodes",
                "nodeRole": role_arn("eksNodeRole"),
                "subnets": ["subnet-aaaa1111", "subnet-bbbb2222"],
                "tags": created_with(),
            }),
            "nodegroupArn",
            format!("/clusters/{cluster_path}/node-groups/tagged-nodes"),
            "nodegroup",
        ),
        (
            "a Fargate profile",
            format!("/clusters/{cluster_path}/fargate-profiles"),
            serde_json::json!({
                "fargateProfileName": "tagged-profile",
                "podExecutionRoleArn": role_arn("eksFargatePodExecutionRole"),
                "subnets": ["subnet-aaaa1111"],
                "selectors": [{ "namespace": "default" }],
                "tags": created_with(),
            }),
            "fargateProfileArn",
            format!("/clusters/{cluster_path}/fargate-profiles/tagged-profile"),
            "fargateProfile",
        ),
        (
            "an add-on",
            format!("/clusters/{cluster_path}/addons"),
            serde_json::json!({ "addonName": "vpc-cni", "tags": created_with() }),
            "addonArn",
            format!("/clusters/{cluster_path}/addons/vpc-cni"),
            "addon",
        ),
        (
            "an EKS Pod Identity association",
            format!("/clusters/{cluster_path}/pod-identity-associations"),
            serde_json::json!({
                "namespace": "default",
                "serviceAccount": "tagged",
                "roleArn": role_arn("eksPodIdentityRole"),
                "tags": created_with(),
            }),
            "associationArn",
            String::new(),
            "association",
        ),
        (
            "an access entry",
            format!("/clusters/{cluster_path}/access-entries"),
            serde_json::json!({
                "principalArn": role_arn("taggedPrincipal"),
                "tags": created_with(),
            }),
            "accessEntryArn",
            format!(
                "/clusters/{cluster_path}/access-entries/{}",
                encode_label(&role_arn("taggedPrincipal"))
            ),
            "accessEntry",
        ),
    ];

    for (case, uri, body, arn_member, describe_uri, payload) in children {
        let created = post(router, &uri, &body).await;
        assert_eq!(
            created.status, 200,
            "setup {case:?}: the create must succeed, got {created:?}"
        );
        let arn = created.body[payload][arn_member]
            .as_str()
            .unwrap_or_else(|| panic!("setup {case:?}: expected {arn_member}, got {created:?}"))
            .to_string();
        // An association is addressed by the id its create minted, so its describe
        // path is built from the answer rather than written above.
        let describe_uri = if describe_uri.is_empty() {
            let id = created.body[payload]["associationId"]
                .as_str()
                .expect("the association's id");
            format!("/clusters/{cluster_path}/pod-identity-associations/{id}")
        } else {
            describe_uri
        };
        rows.push(Resource {
            case,
            arn,
            describe_uri,
            payload,
        });
    }
    rows
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> (u16, serde_json::Value) {
    let answer = get(router, &format!("/tags/{}", encode_label(arn))).await;
    (answer.status, answer.body["tags"].clone())
}

// ---------------------------------------------------------------------------
// RULE: every resource this service mints an ARN for is taggable through that ARN,
// and the tags live on the resource itself.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_resource_this_service_mints_an_arn_for_is_taggable_through_it() {
    let router = emulator();

    for resource in fixtures(&router).await {
        let case = resource.case;
        let arn = resource.arn.as_str();
        let path = format!("/tags/{}", encode_label(arn));

        // The create-time tags are what the listing reports before anything else is
        // asked: the ARN the create answered with addresses the resource the create
        // made.
        let (status, tags) = listed_tags(&router, arn).await;
        assert_eq!(
            (status, &tags),
            (200, &created_with()),
            "case {case:?}: the listing must report the create's own tags for {arn}"
        );

        // TagResource MERGES: the key the create set survives, the new keys are
        // added, and a key the request repeats is overwritten.
        let tagged = post(
            &router,
            &path,
            &serde_json::json!({
                "tags": { "owner": "sre", "env": "test", "unicode": "ünïcödé-✓" }
            }),
        )
        .await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: TagResource must succeed for {arn}, got {tagged:?}"
        );
        let merged = serde_json::json!({
            "owner": "sre",
            "env": "test",
            "unicode": "ünïcödé-✓",
        });
        let (status, tags) = listed_tags(&router, arn).await;
        assert_eq!(
            (status, &tags),
            (200, &merged),
            "case {case:?}: the listing must report the merged map for {arn}"
        );

        // And the RESOURCE carries them: its own describe reports the same map, which
        // is what makes this one record rather than a tag store beside it.
        let read = get(&router, &resource.describe_uri).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the resource must read back, got {read:?}"
        );
        assert_eq!(
            read.body[resource.payload]["tags"], merged,
            "case {case:?}: the resource itself must carry the tags, got {read:?}"
        );

        // UntagResource removes exactly the keys it names.
        let untagged = call(
            &router,
            REGION,
            "DELETE",
            &format!("{path}?tagKeys=env&tagKeys=unicode"),
            "",
        )
        .await;
        assert_eq!(
            untagged.status, 200,
            "case {case:?}: UntagResource must succeed for {arn}, got {untagged:?}"
        );
        let (status, tags) = listed_tags(&router, arn).await;
        assert_eq!(
            (status, &tags),
            (200, &serde_json::json!({ "owner": "sre" })),
            "case {case:?}: only the named keys may be removed from {arn}"
        );
    }
}

// ---------------------------------------------------------------------------
// CONTROL: widening WHICH ARNs resolve did not widen what resolves to nothing — a
// well-formed ARN of this service that no resource carries is still the not-found,
// and a value that is not one at all is still the bad request.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_no_resource_carries_is_still_not_found() {
    let router = emulator();
    // One real resource of every kind exists, so a miss below is about the ARN the
    // row carries rather than about an empty store.
    let held = fixtures(&router).await;
    let account = "000000000000";

    // The rows differ along which resource KIND the ARN names and how it misses: a
    // name no resource of that kind has, and the ARN of a real resource with its
    // Region changed, so none of them resolves while all of them are well formed.
    let mut rows: Vec<(String, String)> = vec![
        (
            "a cluster that was never created".to_string(),
            format!("arn:aws:eks:{REGION}:{account}:cluster/ghost"),
        ),
        (
            "a node group of a cluster that does not exist".to_string(),
            format!("arn:aws:eks:{REGION}:{account}:nodegroup/ghost/nodes/0123abcd"),
        ),
        (
            "a pod-identity association that was never created".to_string(),
            format!(
                "arn:aws:eks:{REGION}:{account}:podidentityassociation/{CLUSTER}/a-0123456789abcdef"
            ),
        ),
    ];
    for resource in &held {
        rows.push((
            format!("{}, in another Region", resource.case),
            resource.arn.replacen(REGION, "eu-west-1", 1),
        ));
    }

    for (case, arn) in rows {
        let path = format!("/tags/{}", encode_label(&arn));
        for (operation, answer) in [
            (
                "ListTagsForResource",
                get(&router, &path).await,
            ),
            (
                "TagResource",
                post(&router, &path, &serde_json::json!({ "tags": { "k": "v" } })).await,
            ),
            (
                "UntagResource",
                call(&router, REGION, "DELETE", &format!("{path}?tagKeys=k"), "").await,
            ),
        ] {
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (404, "NotFoundException"),
                "case {case:?} / {operation}: expected the not-found, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                TAG_RESOURCE_NOT_FOUND_MESSAGE,
                "case {case:?} / {operation}: expected the one sentence, got {answer:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_value_that_is_not_a_resource_arn_of_this_service_is_still_a_bad_request() {
    let router = emulator();

    // The rows differ along how the value fails to be a resource ARN of this
    // service: another vendor's ARN, an ARN with too few fields, and text that is no
    // ARN at all.
    let rows = [
        (
            "another vendor's ARN",
            "arn:aws:s3:::a-bucket".to_string(),
        ),
        (
            "an ARN missing its resource",
            format!("arn:aws:eks:{REGION}:000000000000"),
        ),
        ("a value that is not an ARN", "not-an-arn".to_string()),
    ];

    for (case, arn) in rows {
        let answer = get(&router, &format!("/tags/{}", encode_label(&arn))).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected the bad request, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            INVALID_RESOURCE_ARN_MESSAGE,
            "case {case:?}: expected the one sentence, got {answer:?}"
        );
    }
}
