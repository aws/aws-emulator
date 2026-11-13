// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for three rules about a cluster Amazon EKS does NOT manage,
//! driven over the wire through the same `MultiServiceRouter` the production
//! binary mounts:
//!
//! 1. every operation whose subject is a resource of the Amazon EKS-managed plane
//!    of a cluster refuses a cluster `RegisterCluster` connected with HTTP 400
//!    `ClientException`, as soon as the cluster is resolved;
//! 2. `ListClusters` matches its `include` projection as an exact set membership
//!    test, refusing every other token set — a case variant included — with one
//!    `InvalidRequestException`;
//! 3. `DeleteAccessEntry` removes the entry it names and answers an empty 200, so
//!    a later read is a 404 and the principal can be granted access again.
//!
//! Each suite asserts its rule over a TABLE whose rows differ along what the rule
//! generalises over — which operation is called and with what shape of request,
//! which token set is sent, which principal and access-entry type the entry
//! carries — rather than over one recorded request, and every assertion names its
//! case and prints the actual value.
//!
//! The wire harness is the crate's one copy in `tests/all/wire.rs` and the cluster
//! helpers are `cluster_lifecycle`'s: no suite grows its own client.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CLUSTER_ACTIVATION_DELAY, CONNECTED_CLUSTER_REFUSAL_MESSAGE, LIST_INCLUDE_ALL,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{
    access_entry_cluster_request, create, describe, list, minimal_request, names_of, REGION,
};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The emulator's own canonical account id, which every ARN it mints carries.
const ACCOUNT: &str = "000000000000";

/// An IAM role in the single account this emulator serves — what a connector
/// configuration, a node role and an access-entry principal are all shaped like.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// A well-formed EKS Pod Identity association id (`^a-[0-9a-z]{17}$`), so an
/// association-scoped probe is refused for the CLUSTER it names rather than for
/// the shape of its id.
const ASSOCIATION_ID: &str = "a-abcdefghij1234567";

/// A well-formed cluster update id, for the update-scoped probes.
const UPDATE_ID: &str = "00000000-0000-4000-8000-000000000000";

async fn register(router: &MultiServiceRouter, name: &str) -> Answer {
    let body = serde_json::json!({
        "name": name,
        "connectorConfig": {
            "roleArn": role_arn("AmazonEKSConnectorRole"),
            "provider": "OTHER",
        },
    });
    call(
        router,
        REGION,
        "POST",
        "/cluster-registrations",
        &body.to_string(),
    )
    .await
}

/// One probe of the EKS-managed plane: the operation, and the request it makes
/// against the cluster named `cluster`.
struct Probe {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: String,
}

/// Every operation of the Amazon EKS-managed plane this crate implements, as a
/// request against `cluster`.
///
/// The rows differ along the plane's resource (add-on, node group, access entry,
/// access policy, identity-provider configuration, pod identity association,
/// capability, certificate authority, control-plane update, cluster insights) and
/// along the shape of the call (create, read, list, update, delete), because the
/// rule is about the cluster, not about one operation. The scaffolded operations
/// of the same plane (`ListAddons`, `ListNodegroups`, `UpdateAccessEntry`, … — each
/// with its own open stub issue) still answer 501 and so are not probed here.
///
/// Every body satisfies the modelled constraints, so what the emulator answers
/// comes from the handler and not from the boundary validator — with ONE
/// deliberate exception, noted on the row that carries it.
fn probes(cluster: &str) -> Vec<Probe> {
    let label = encode_label(cluster);
    let principal = role_arn("AccessEntryPrincipal");
    let entry = encode_label(&principal);
    let policy = "arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy";
    let probe = |case: &'static str, method: &'static str, uri: String, body: String| Probe {
        case,
        method,
        uri,
        body,
    };
    vec![
        probe(
            "CreateAddon",
            "POST",
            format!("/clusters/{label}/addons"),
            serde_json::json!({ "addonName": "vpc-cni" }).to_string(),
        ),
        probe(
            "DescribeAddon",
            "GET",
            format!("/clusters/{label}/addons/vpc-cni"),
            String::new(),
        ),
        probe(
            "DeleteAddon",
            "DELETE",
            format!("/clusters/{label}/addons/vpc-cni"),
            String::new(),
        ),
        probe(
            "CreateNodegroup",
            "POST",
            format!("/clusters/{label}/node-groups"),
            serde_json::json!({
                "nodegroupName": "probe-nodegroup",
                "subnets": ["subnet-aaaa1111"],
                "nodeRole": role_arn("eksNodeRole"),
            })
            .to_string(),
        ),
        probe(
            "DescribeNodegroup",
            "GET",
            format!("/clusters/{label}/node-groups/probe-nodegroup"),
            String::new(),
        ),
        probe(
            // Its payload is SOUND — one selector with a namespace — because the
            // selectors a Fargate profile declares with are judged from the request
            // alone and so are reported before this gate (pinned by
            // `fargate_profiles`); the gate is what answers a request there is
            // nothing wrong with.
            "CreateFargateProfile",
            "POST",
            format!("/clusters/{label}/fargate-profiles"),
            serde_json::json!({
                "fargateProfileName": "probe-fargate-profile",
                "podExecutionRoleArn": role_arn("eksFargatePodExecutionRole"),
                "selectors": [{ "namespace": "default" }],
            })
            .to_string(),
        ),
        probe(
            "DescribeFargateProfile",
            "GET",
            format!("/clusters/{label}/fargate-profiles/probe-fargate-profile"),
            String::new(),
        ),
        probe(
            "CreateAccessEntry",
            "POST",
            format!("/clusters/{label}/access-entries"),
            serde_json::json!({ "principalArn": principal }).to_string(),
        ),
        probe(
            "DescribeAccessEntry",
            "GET",
            format!("/clusters/{label}/access-entries/{entry}"),
            String::new(),
        ),
        probe(
            "DeleteAccessEntry",
            "DELETE",
            format!("/clusters/{label}/access-entries/{entry}"),
            String::new(),
        ),
        probe(
            "ListAccessEntries",
            "GET",
            format!("/clusters/{label}/access-entries"),
            String::new(),
        ),
        probe(
            "AssociateAccessPolicy",
            "POST",
            format!("/clusters/{label}/access-entries/{entry}/access-policies"),
            serde_json::json!({
                "policyArn": policy,
                "accessScope": { "type": "cluster" },
            })
            .to_string(),
        ),
        probe(
            "ListAssociatedAccessPolicies",
            "GET",
            format!("/clusters/{label}/access-entries/{entry}/access-policies"),
            String::new(),
        ),
        probe(
            // The one row whose body is deliberately WRONG — the `keyArn` is not a
            // KMS key reference — because the recorded instance of this gate is
            // exactly that call: the gate is a fact about the cluster, so it is
            // answered before the body is judged against it.
            "AssociateEncryptionConfig with a malformed keyArn",
            "POST",
            format!("/clusters/{label}/encryption-config/associate"),
            serde_json::json!({
                "encryptionConfig": [{ "provider": { "keyArn": "not-a-kms-key" } }],
            })
            .to_string(),
        ),
        probe(
            "UpdateClusterConfig",
            "POST",
            format!("/clusters/{label}/update-config"),
            serde_json::json!({ "deletionProtection": true }).to_string(),
        ),
        probe(
            "UpdateClusterVersion",
            "POST",
            format!("/clusters/{label}/updates"),
            serde_json::json!({ "version": "1.33" }).to_string(),
        ),
        probe(
            "DescribeUpdate",
            "GET",
            format!("/clusters/{label}/updates/{UPDATE_ID}"),
            String::new(),
        ),
        probe(
            "ListUpdates",
            "GET",
            format!("/clusters/{label}/updates"),
            String::new(),
        ),
        probe(
            "CancelUpdate",
            "POST",
            format!("/clusters/{label}/updates/{UPDATE_ID}/cancel-update"),
            String::new(),
        ),
        probe(
            "AssociateIdentityProviderConfig",
            "POST",
            format!("/clusters/{label}/identity-provider-configs/associate"),
            serde_json::json!({
                "oidc": {
                    "identityProviderConfigName": "probe-idp",
                    "issuerUrl": "https://example.com",
                    "clientId": "probe-client",
                },
            })
            .to_string(),
        ),
        probe(
            "DescribeIdentityProviderConfig",
            "POST",
            format!("/clusters/{label}/identity-provider-configs/describe"),
            serde_json::json!({
                "identityProviderConfig": { "type": "oidc", "name": "probe-idp" },
            })
            .to_string(),
        ),
        probe(
            "CreatePodIdentityAssociation",
            "POST",
            format!("/clusters/{label}/pod-identity-associations"),
            serde_json::json!({
                "namespace": "default",
                "serviceAccount": "probe-sa",
                "roleArn": role_arn("probePodIdentityRole"),
            })
            .to_string(),
        ),
        probe(
            "DescribePodIdentityAssociation",
            "GET",
            format!("/clusters/{label}/pod-identity-associations/{ASSOCIATION_ID}"),
            String::new(),
        ),
        probe(
            "UpdatePodIdentityAssociation",
            "POST",
            format!("/clusters/{label}/pod-identity-associations/{ASSOCIATION_ID}"),
            serde_json::json!({ "roleArn": role_arn("probePodIdentityRole") }).to_string(),
        ),
        probe(
            "DeletePodIdentityAssociation",
            "DELETE",
            format!("/clusters/{label}/pod-identity-associations/{ASSOCIATION_ID}"),
            String::new(),
        ),
        probe(
            "ListPodIdentityAssociations",
            "GET",
            format!("/clusters/{label}/pod-identity-associations"),
            String::new(),
        ),
        probe(
            "CreateCapability",
            "POST",
            format!("/clusters/{label}/capabilities"),
            serde_json::json!({
                "capabilityName": "probe-capability",
                "type": "ACK",
                "roleArn": role_arn("probeCapabilityRole"),
                "deletePropagationPolicy": "RETAIN",
            })
            .to_string(),
        ),
        probe(
            "DescribeCapability",
            "GET",
            format!("/clusters/{label}/capabilities/probe-capability"),
            String::new(),
        ),
        probe(
            "DeleteCapability",
            "DELETE",
            format!("/clusters/{label}/capabilities/probe-capability"),
            String::new(),
        ),
        probe(
            "ListCapabilities",
            "GET",
            format!("/clusters/{label}/capabilities"),
            String::new(),
        ),
        probe(
            "CreateCertificateAuthority",
            "POST",
            format!("/clusters/{label}/certificate-authorities"),
            "{}".to_string(),
        ),
        probe(
            "DescribeCertificateAuthority",
            "GET",
            format!("/clusters/{label}/certificate-authorities/probe-ca"),
            String::new(),
        ),
        probe(
            "ActivateCertificateAuthority",
            "POST",
            format!("/clusters/{label}/certificate-authorities/probe-ca/activate"),
            "{}".to_string(),
        ),
        probe(
            "ListCertificateAuthorities",
            "GET",
            format!("/clusters/{label}/certificate-authorities"),
            String::new(),
        ),
        probe(
            "StartInsightsRefresh",
            "POST",
            format!("/clusters/{label}/insights-refresh"),
            String::new(),
        ),
        probe(
            "DescribeInsightsRefresh",
            "GET",
            format!("/clusters/{label}/insights-refresh"),
            String::new(),
        ),
    ]
}

async fn run(router: &MultiServiceRouter, probe: &Probe) -> Answer {
    call(router, REGION, probe.method, &probe.uri, &probe.body).await
}

// ---------------------------------------------------------------------------
// RULE: an operation whose subject is a resource of the Amazon EKS-managed plane
// of a cluster does not apply to a cluster Amazon EKS does not manage (one
// `RegisterCluster` connected), so it is refused — as soon as the cluster is
// resolved, and whatever the request body or the child resource it names — with
// HTTP 400 `ClientException` and the one fixed sentence that refusal carries. A
// cluster the region does not hold is still a 404, and an Amazon EKS cluster is
// never answered this refusal.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_eks_managed_plane_operation_refuses_a_connected_cluster() {
    let router = emulator();

    let connected = "gate-connected";
    let registered = register(&router, connected).await;
    assert_eq!(
        registered.status, 200,
        "setup: registering {connected:?} must succeed, got {registered:?}"
    );

    for probe in probes(connected) {
        let refused = run(&router, &probe).await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400 on a connected cluster, got {refused:?}",
            probe.case
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {:?}: expected ClientException, got {:?} ({refused:?})",
            probe.case,
            error_code(&refused)
        );
        assert_eq!(
            refused.message(),
            CONNECTED_CLUSTER_REFUSAL_MESSAGE,
            "case {:?}: expected the connected-cluster refusal sentence, got {:?}",
            probe.case,
            refused.message()
        );

        // The refusal is PERMANENT and wrote nothing, so replaying the very same
        // call is answered identically rather than conflicting with something the
        // first attempt left behind.
        let replayed = run(&router, &probe).await;
        assert_eq!(
            (replayed.status, error_code(&replayed), replayed.message()),
            (refused.status, error_code(&refused), refused.message()),
            "case {:?}: a replayed refused call must answer identically, got {replayed:?}",
            probe.case
        );
    }

    // And the registration itself is untouched by every one of those calls: it
    // still describes, still reports its connector configuration, and is still
    // listed under the projection that asks for connected clusters.
    let described = describe(&router, connected).await;
    assert_eq!(
        described.status, 200,
        "a connected cluster still describes after the gate refused every call, got {described:?}"
    );
    assert!(
        described.cluster()["connectorConfig"]["roleArn"].is_string(),
        "the stored registration must still report its connector configuration, got {:?}",
        described.cluster()
    );
    let listed = list(&router, &format!("include={LIST_INCLUDE_ALL}")).await;
    assert!(
        names_of(&listed).contains(&connected.to_string()),
        "the connected cluster must still be listed, got {:?}",
        names_of(&listed)
    );
}

#[tokio::test]
async fn an_unknown_cluster_is_still_not_found_and_an_eks_cluster_is_never_gated() {
    let router = emulator();

    let managed = "gate-eks-cluster";
    let created = create(&router, &minimal_request(managed)).await;
    assert_eq!(
        created.status, 200,
        "setup: creating {managed:?} must succeed, got {created:?}"
    );

    // CONTROL 1 — the gate did not swallow the not-found: a cluster this region
    // does not hold is still reported as the missing resource.
    for probe in probes("gate-no-such-cluster") {
        let answer = run(&router, &probe).await;
        assert_eq!(
            answer.status, 404,
            "case {:?}: an unknown cluster is still a 404, got {answer:?}",
            probe.case
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException, got {:?} ({answer:?})",
            probe.case,
            error_code(&answer)
        );
    }

    // CONTROL 2 — an Amazon EKS cluster is never answered the gate, whatever else
    // the call is refused for (a child it does not hold, a status it is not in, a
    // body it cannot act on).
    for probe in probes(managed) {
        let answer = run(&router, &probe).await;
        assert_ne!(
            error_code(&answer),
            "ClientException",
            "case {:?}: an Amazon EKS cluster must never be answered the connected-cluster \
             refusal, got {answer:?}",
            probe.case
        );
        assert_ne!(
            answer.message(),
            CONNECTED_CLUSTER_REFUSAL_MESSAGE,
            "case {:?}: an Amazon EKS cluster must never carry the connected-cluster sentence, \
             got {answer:?}",
            probe.case
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `ListClusters`'s `include` names the listing's projection, and its token
// set is matched as an exact set membership test that is never case-folded —
// "'all' must be in lowercase otherwise an error occurs" — so only the token set
// the member documents projects connected clusters, and every other set is
// refused with one HTTP 400 `InvalidRequestException`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_include_token_set_outside_the_documented_one_is_refused() {
    let router = emulator();

    let managed = ["include-eks-a", "include-eks-b"];
    for name in managed {
        let created = create(&router, &minimal_request(name)).await;
        assert_eq!(
            created.status, 200,
            "setup: creating {name:?} must succeed, got {created:?}"
        );
    }
    let connected = "include-connected";
    let registered = register(&router, connected).await;
    assert_eq!(
        registered.status, 200,
        "setup: registering {connected:?} must succeed, got {registered:?}"
    );

    // The rows differ along HOW the token set falls outside the documented one:
    // the case of the documented token, an undocumented token, a token that names
    // the projection in words, and a set that pairs the documented token with one
    // of those — a set is projected only if EVERY member of it is documented.
    let refused = [
        "ALL",
        "All",
        "aLl",
        "bogus",
        "eks",
        "connected",
        "all,bogus",
        "all&include=ALL",
    ];
    for value in refused {
        let answer = list(&router, &format!("include={value}")).await;
        assert_eq!(
            answer.status, 400,
            "case include={value:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidRequestException",
            "case include={value:?}: expected InvalidRequestException, got {:?} ({answer:?})",
            error_code(&answer)
        );
        // One refusal for every unacceptable set: the sentence names the parameter
        // and nothing the caller sent.
        assert_eq!(
            answer.message(),
            "One or more values in the 'include' parameter is not valid",
            "case include={value:?}: expected the fixed include refusal, got {:?}",
            answer.message()
        );
    }

    // And the documented token set still projects: with it, the connected cluster
    // is listed; without it (absent, or empty, which is "blank"), it is not.
    let mut everything: Vec<String> = managed.iter().map(|name| (*name).to_owned()).collect();
    everything.push(connected.to_owned());
    everything.sort();
    let mut only_managed: Vec<String> = managed.iter().map(|name| (*name).to_owned()).collect();
    only_managed.sort();
    for (case, query, expected) in [
        ("the documented token", format!("include={LIST_INCLUDE_ALL}"), &everything),
        ("no include at all", String::new(), &only_managed),
        ("an empty include, which is blank", "include=".to_string(), &only_managed),
        (
            "the documented token twice, which is the same set",
            format!("include={LIST_INCLUDE_ALL}&include={LIST_INCLUDE_ALL}"),
            &everything,
        ),
    ] {
        let answer = list(&router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {answer:?}"
        );
        let actual = names_of(&answer);
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a listing of a resource of the EKS-managed plane reports the names that
// resource's own map holds FOR ONE CLUSTER, in sorted order, paged with a token
// that resumes after the last name returned — so a walk at any page size yields
// every name exactly once and the last page carries no token.
// ---------------------------------------------------------------------------

async fn list_in_cluster(router: &MultiServiceRouter, uri: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        uri.to_string()
    } else {
        format!("{uri}?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

/// Walk every page of `uri` under `member`, at `page_size`, and return the names
/// in the order the listing reported them.
async fn walk(
    router: &MultiServiceRouter,
    uri: &str,
    member: &str,
    page_size: Option<usize>,
) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut parts: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            parts.push(format!("maxResults={size}"));
        }
        if let Some(next) = &token {
            parts.push(format!("nextToken={next}"));
        }
        let answer = list_in_cluster(router, uri, &parts.join("&")).await;
        assert_eq!(
            answer.status, 200,
            "case {uri} (page size {page_size:?}): expected HTTP 200, got {answer:?}"
        );
        let page: Vec<String> = answer.body[member]
            .as_array()
            .unwrap_or_else(|| panic!("expected a {member} list, got {:?}", answer.body))
            .iter()
            .map(|name| name.as_str().unwrap_or_default().to_owned())
            .collect();
        if let Some(size) = page_size {
            assert!(
                page.len() <= size,
                "case {uri} (page size {page_size:?}): a page must not exceed maxResults, got \
                 {page:?}"
            );
        }
        walked.extend(page);
        token = answer.body["nextToken"].as_str().map(str::to_owned);
        if token.is_none() {
            break;
        }
        assert!(
            walked.len() <= 100,
            "case {uri} (page size {page_size:?}): paging must terminate, walked {walked:?}"
        );
    }
    walked
}

#[tokio::test]
async fn the_managed_plane_listings_page_every_name_the_cluster_holds() {
    let router = emulator();
    let cluster = "listing-cluster";
    let created = create(&router, &minimal_request(cluster)).await;
    assert_eq!(
        created.status, 200,
        "setup: creating {cluster:?} must succeed, got {created:?}"
    );
    // An add-on and a node group are both created on an ACTIVE cluster, so the
    // span the clock owes the new cluster is waited out first.
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + std::time::Duration::from_millis(250)).await;

    // The names are deliberately NOT in creation order, so a listing that reports
    // insertion order rather than sorted order fails.
    let addons = ["vpc-cni", "coredns", "aws-ebs-csi-driver"];
    for addon in addons {
        let answer = call(
            &router,
            REGION,
            "POST",
            &format!("/clusters/{}/addons", encode_label(cluster)),
            &serde_json::json!({ "addonName": addon }).to_string(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "setup: creating add-on {addon:?} must succeed, got {answer:?}"
        );
    }
    let nodegroups = ["workers-b", "workers-a", "workers-c"];
    for nodegroup in nodegroups {
        let answer = call(
            &router,
            REGION,
            "POST",
            &format!("/clusters/{}/node-groups", encode_label(cluster)),
            &serde_json::json!({
                "nodegroupName": nodegroup,
                "subnets": ["subnet-aaaa1111"],
                "nodeRole": role_arn("eksNodeRole"),
            })
            .to_string(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "setup: creating node group {nodegroup:?} must succeed, got {answer:?}"
        );
    }

    // An EMPTY cluster of the same store, so a listing that reports another
    // cluster's children fails.
    let empty = "listing-empty-cluster";
    let created = create(&router, &minimal_request(empty)).await;
    assert_eq!(
        created.status, 200,
        "setup: creating {empty:?} must succeed, got {created:?}"
    );

    let mut expected_addons: Vec<String> = addons.iter().map(|name| (*name).to_owned()).collect();
    expected_addons.sort();
    let mut expected_nodegroups: Vec<String> =
        nodegroups.iter().map(|name| (*name).to_owned()).collect();
    expected_nodegroups.sort();

    // The rows differ along the listing, the cluster (one holding three children,
    // one holding none) and the page size, including one below, at and above the
    // number of children.
    let cases: Vec<(&str, String, &str, &Vec<String>)> = vec![
        (
            "add-ons of the cluster that holds them",
            format!("/clusters/{}/addons", encode_label(cluster)),
            "addons",
            &expected_addons,
        ),
        (
            "node groups of the cluster that holds them",
            format!("/clusters/{}/node-groups", encode_label(cluster)),
            "nodegroups",
            &expected_nodegroups,
        ),
    ];
    let none: Vec<String> = Vec::new();
    for (case, uri, member, expected) in cases {
        for page_size in [None, Some(1), Some(2), Some(3), Some(100)] {
            let walked = walk(&router, &uri, member, page_size).await;
            assert_eq!(
                &walked, expected,
                "case {case:?} (page size {page_size:?}): expected {expected:?}, got {walked:?}"
            );
        }
    }
    for (case, uri, member) in [
        (
            "add-ons of a cluster that holds none",
            format!("/clusters/{}/addons", encode_label(empty)),
            "addons",
        ),
        (
            "node groups of a cluster that holds none",
            format!("/clusters/{}/node-groups", encode_label(empty)),
            "nodegroups",
        ),
    ] {
        let walked = walk(&router, &uri, member, None).await;
        assert_eq!(
            walked, none,
            "case {case:?}: a cluster that holds none lists nothing, got {walked:?}"
        );
    }

    // A `nextToken` this listing did not mint is a bad parameter, not an empty
    // page: the rows are a string that is no token at all, and a well-formed token
    // the OTHER listing minted.
    let addons_token = list_in_cluster(
        &router,
        &format!("/clusters/{}/addons", encode_label(cluster)),
        "maxResults=1",
    )
    .await
    .body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a nextToken on a 1-row add-on page"))
        .to_owned();
    for (case, uri, token) in [
        (
            "a token this service never minted, on ListAddons",
            format!("/clusters/{}/addons", encode_label(cluster)),
            "not-a-token".to_string(),
        ),
        (
            "a token minted by the add-on listing, on ListNodegroups",
            format!("/clusters/{}/node-groups", encode_label(cluster)),
            addons_token,
        ),
    ] {
        let answer = list_in_cluster(&router, &uri, &format!("nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `DeregisterCluster` removes a CONNECTED cluster from this service —
// "deregisters a connected cluster to remove it from the Amazon EKS control
// plane" — answering with the record it removed; a cluster that is not a
// registration has nothing to deregister (`ClientException`), and a name the
// region does not hold is a 404. It is how a registration is removed once every
// operation of the EKS-managed plane is gated for it.
// ---------------------------------------------------------------------------

async fn deregister(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &format!("/cluster-registrations/{}", encode_label(name)),
        "",
    )
    .await
}

#[tokio::test]
async fn deregistering_removes_a_registration_and_refuses_anything_else() {
    let router = emulator();

    // The rows differ along what the name addresses: a registration (which
    // deregisters), an Amazon EKS cluster (which is not a registration at all) and
    // a name this region does not hold.
    let registered = "deregister-connected";
    let answer = register(&router, registered).await;
    assert_eq!(
        answer.status, 200,
        "setup: registering {registered:?} must succeed, got {answer:?}"
    );
    let managed = "deregister-eks-cluster";
    let answer = create(&router, &minimal_request(managed)).await;
    assert_eq!(
        answer.status, 200,
        "setup: creating {managed:?} must succeed, got {answer:?}"
    );

    let deregistered = deregister(&router, registered).await;
    assert_eq!(
        deregistered.status, 200,
        "a registration deregisters, got {deregistered:?}"
    );
    assert_eq!(
        deregistered.cluster()["name"],
        serde_json::json!(registered),
        "the answer reports the cluster it removed, got {:?}",
        deregistered.cluster()
    );
    assert!(
        deregistered.cluster()["connectorConfig"]["roleArn"].is_string(),
        "the answer reports the registration it removed, got {:?}",
        deregistered.cluster()
    );
    assert_eq!(
        deregistered.cluster()["status"],
        serde_json::json!("DELETING"),
        "the removed registration reports its final status, got {:?}",
        deregistered.cluster()
    );

    // It is gone: it no longer describes, it is no longer listed under the
    // projection that asks for connected clusters, and deregistering it again is
    // the not-found a name the region does not hold gets.
    let described = describe(&router, registered).await;
    assert_eq!(
        described.status, 404,
        "a deregistered cluster is gone, got {described:?}"
    );
    let listed = list(&router, &format!("include={LIST_INCLUDE_ALL}")).await;
    assert!(
        !names_of(&listed).contains(&registered.to_string()),
        "a deregistered cluster is no longer listed, got {:?}",
        names_of(&listed)
    );

    for (case, name) in [
        ("a registration that was already deregistered", registered),
        ("a name the region does not hold", "deregister-no-such"),
    ] {
        let answer = deregister(&router, name).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    // An Amazon EKS cluster is not a registration, so it is refused — and it is
    // still there afterwards.
    let refused = deregister(&router, managed).await;
    assert_eq!(
        refused.status, 400,
        "an Amazon EKS cluster has nothing to deregister, got {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ClientException",
        "expected ClientException, got {refused:?}"
    );
    let still_there = describe(&router, managed).await;
    assert_eq!(
        still_there.status, 200,
        "a refused deregistration removes nothing, got {still_there:?}"
    );

    // And the name a deregistration freed can be registered again.
    let again = register(&router, registered).await;
    assert_eq!(
        again.status, 200,
        "the name must be free to register again, got {again:?}"
    );
}
// from the cluster that holds it and answers an empty 200 — so a later read of
// that entry is a `ResourceNotFoundException`, the principal can be granted
// access again, the policies associated to the entry go with it, and the other
// entries of the cluster are untouched. Deleting an entry that is not there is
// the same 404 as reading it.
// ---------------------------------------------------------------------------

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

async fn describe_entry(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/access-entries/{}",
            encode_label(cluster),
            encode_label(principal)
        ),
        "",
    )
    .await
}

async fn delete_entry(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &format!(
            "/clusters/{}/access-entries/{}",
            encode_label(cluster),
            encode_label(principal)
        ),
        "",
    )
    .await
}

#[tokio::test]
async fn deleting_an_access_entry_removes_it_and_lets_it_be_created_again() {
    let router = emulator();
    let cluster = "delete-entry-cluster";
    // The access-entry API is served only by a cluster whose `authenticationMode`
    // includes it (`cluster_lifecycle::access_entry_cluster_request`).
    let created = create(&router, &access_entry_cluster_request(cluster)).await;
    assert_eq!(
        created.status, 200,
        "setup: creating {cluster:?} must succeed, got {created:?}"
    );

    // The rows differ along the principal the entry is keyed by (a role, a role
    // with a path, an IAM user) and along the entry's own type — "deleting an
    // access entry of a type other than Standard can cause your cluster to
    // function improperly", so every type is deletable, not just the default one.
    // The type names are the members `CreateAccessEntryRequest$type` documents.
    let cases: Vec<(&str, String, serde_json::Value)> = vec![
        (
            "a role with the default type",
            role_arn("DeletableStandardRole"),
            serde_json::json!({}),
        ),
        (
            "a role at a path",
            format!("arn:aws:iam::{ACCOUNT}:role/service-role/DeletableNestedRole"),
            serde_json::json!({ "type": "STANDARD" }),
        ),
        (
            "an IAM user",
            format!("arn:aws:iam::{ACCOUNT}:user/deletable-user"),
            serde_json::json!({ "type": "STANDARD" }),
        ),
        (
            "an EC2 Linux node role",
            role_arn("DeletableEc2LinuxRole"),
            serde_json::json!({ "type": "EC2_LINUX" }),
        ),
        (
            "a Fargate Linux node role",
            role_arn("DeletableFargateRole"),
            serde_json::json!({ "type": "FARGATE_LINUX" }),
        ),
    ];

    for (case, principal, extra) in &cases {
        let mut body = serde_json::json!({ "principalArn": principal });
        for (key, value) in extra.as_object().into_iter().flatten() {
            body[key] = value.clone();
        }
        let created = create_entry(&router, cluster, &body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: creating the entry must succeed, got {created:?}"
        );

        let before = describe_entry(&router, cluster, principal).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the entry must read back before the delete, got {before:?}"
        );

        let deleted = delete_entry(&router, cluster, principal).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected an empty 200 from the delete, got {deleted:?}"
        );
        assert!(
            deleted.body.is_null() || deleted.body == serde_json::json!({}),
            "case {case:?}: the delete answers an empty body, got {:?}",
            deleted.body
        );

        let after = describe_entry(&router, cluster, principal).await;
        assert_eq!(
            after.status, 404,
            "case {case:?}: the entry must be gone after the delete, got {after:?}"
        );
        assert_eq!(
            error_code(&after),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {after:?}"
        );

        // Deleting it again is the same not-found: the first delete removed it.
        let again = delete_entry(&router, cluster, principal).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: deleting an absent entry is a 404, got {again:?}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {again:?}"
        );

        // "If you delete an access entry in error, you can recreate it": the name
        // is free again, so the create that conflicted before now succeeds.
        let recreated = create_entry(&router, cluster, &body).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the principal must be grantable again, got {recreated:?}"
        );
    }
}

#[tokio::test]
async fn deleting_one_entry_takes_its_policies_and_leaves_the_others() {
    let router = emulator();
    let cluster = "delete-entry-scope";
    // The entries this asserts over are the two created below, so the cluster is
    // created without the creator's own bootstrap access entry
    // (`accessConfig.bootstrapClusterCreatorAdminPermissions`, covered by
    // `cluster_creator_access_entry.rs`).
    let mut request = minimal_request(cluster);
    request["accessConfig"] = serde_json::json!({
        "authenticationMode": "API_AND_CONFIG_MAP",
        "bootstrapClusterCreatorAdminPermissions": false,
    });
    let created = create(&router, &request).await;
    assert_eq!(
        created.status, 200,
        "setup: creating {cluster:?} must succeed, got {created:?}"
    );

    let doomed = role_arn("DoomedRole");
    let survivor = role_arn("SurvivingRole");
    for principal in [&doomed, &survivor] {
        let created =
            create_entry(&router, cluster, &serde_json::json!({ "principalArn": principal })).await;
        assert_eq!(
            created.status, 200,
            "setup: creating the entry for {principal:?} must succeed, got {created:?}"
        );
    }

    // The doomed entry carries an associated access policy, which hangs off the
    // ENTRY: removing the entry removes it too, with no second map left in step.
    let policy = "arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy";
    let associated = call(
        &router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/access-entries/{}/access-policies",
            encode_label(cluster),
            encode_label(&doomed)
        ),
        &serde_json::json!({ "policyArn": policy, "accessScope": { "type": "cluster" } })
            .to_string(),
    )
    .await;
    assert_eq!(
        associated.status, 200,
        "setup: associating {policy:?} must succeed, got {associated:?}"
    );

    let deleted = delete_entry(&router, cluster, &doomed).await;
    assert_eq!(
        deleted.status, 200,
        "the delete must succeed, got {deleted:?}"
    );

    let policies = call(
        &router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/access-entries/{}/access-policies",
            encode_label(cluster),
            encode_label(&doomed)
        ),
        "",
    )
    .await;
    assert_eq!(
        policies.status, 404,
        "the policies of a deleted entry are gone with it, got {policies:?}"
    );

    // The cluster's other entry is untouched, and the listing reports exactly it.
    let survived = describe_entry(&router, cluster, &survivor).await;
    assert_eq!(
        survived.status, 200,
        "the other entry must survive the delete, got {survived:?}"
    );
    let listed = call(
        &router,
        REGION,
        "GET",
        &format!("/clusters/{}/access-entries", encode_label(cluster)),
        "",
    )
    .await;
    let entries: Vec<String> = listed.body["accessEntries"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        entries,
        vec![survivor.clone()],
        "expected only the surviving entry to be listed, got {entries:?}"
    );
}
