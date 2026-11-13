// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH SENTENCE a listing of this service refuses a `nextToken`
//! it did not issue with, driven over the wire through the production
//! `MultiServiceRouter` (`tests/all/wire.rs`).
//!
//! The rule under test: Amazon EKS words a refusal about one request member in the form
//! the API PLANE serving the operation words such a refusal in, so the page-token
//! refusal is a property of the LISTING and not one sentence for the whole service. The
//! Kubernetes access-entry plane names the MEMBER at fault and what is wrong with it —
//! the form it refuses an access entry's `username` with too — while the
//! cluster-management plane answers Amazon EKS's own page-token sentence, which names a
//! member no caller has to correct and covers a token it cannot read and one whose
//! position is gone alike. The class is the same either way (HTTP 400
//! `InvalidParameterException`), each sentence is entirely fixed text that echoes
//! nothing of the request, and the two sentences are NOT interchangeable: one listing of
//! each plane, handed the very same token, answers the sentence of its own plane.
//!
//! The rows span both planes' listings — including ones the issue this rule was written
//! for did not cite — and, for each, how the token misses: a value that is not even
//! hexadecimal, hexadecimal bytes nothing minted, the empty token, and a well-formed
//! token another listing really minted.
//!
//! The access-entry setup (`active_cluster`, `create_entry`, `associate_policy`,
//! `principal_arn`) is `tests/all/refusal_contract.rs`'s; `tests/all.rs` is this
//! binary's crate root, so it is reached through `crate::refusal_contract`.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    access_policy_arn, ACCESS_ENTRY_INVALID_NEXT_TOKEN_MESSAGE, INVALID_NEXT_TOKEN_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::REGION;
use crate::refusal_contract::{active_cluster, associate_policy, create_entry, principal_arn};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The cluster every listing below is addressed on.
const CLUSTER: &str = "plane-wording";

/// The modelled class a token no listing minted is refused as, and its status: a value
/// the caller handed in that this service cannot decode is a malformed PARAMETER.
const MALFORMED_PARAMETER: &str = "InvalidParameterException";
const MALFORMED_PARAMETER_STATUS: u16 = 400;

/// Which API plane serves a listing, which is what decides the wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plane {
    /// The Kubernetes access-entry APIs.
    AccessEntry,
    /// Everything else: the cluster and its managed resources.
    ClusterManagement,
}

impl Plane {
    /// The sentence this plane refuses a token no listing minted with.
    fn sentence(self) -> &'static str {
        match self {
            Self::AccessEntry => ACCESS_ENTRY_INVALID_NEXT_TOKEN_MESSAGE,
            Self::ClusterManagement => INVALID_NEXT_TOKEN_MESSAGE,
        }
    }
}

/// One listing: the operation, the path it is addressed at and the plane serving it.
struct Listing {
    operation: &'static str,
    path: String,
    plane: Plane,
}

/// A listing of each plane, named by the paths the model binds them to.
fn listings(policies_path: &str) -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListAssociatedAccessPolicies",
            path: policies_path.to_string(),
            plane: Plane::AccessEntry,
        },
        Listing {
            operation: "ListAccessEntries",
            path: format!("/clusters/{CLUSTER}/access-entries"),
            plane: Plane::AccessEntry,
        },
        Listing {
            operation: "ListAccessPolicies",
            path: "/access-policies".to_string(),
            plane: Plane::AccessEntry,
        },
        Listing {
            operation: "ListClusters",
            path: "/clusters".to_string(),
            plane: Plane::ClusterManagement,
        },
        Listing {
            operation: "ListNodegroups",
            path: format!("/clusters/{CLUSTER}/node-groups"),
            plane: Plane::ClusterManagement,
        },
        Listing {
            operation: "DescribeClusterVersions",
            path: "/cluster-versions".to_string(),
            plane: Plane::ClusterManagement,
        },
    ]
}

/// How a token misses: the case name and the value the request carries.
fn tokens(minted_elsewhere: &str) -> Vec<(&'static str, String)> {
    vec![
        ("a value that is not hexadecimal", "not-a-token".to_string()),
        (
            "hexadecimal bytes nothing minted",
            "deadbeefdeadbeefdeadbeef".to_string(),
        ),
        ("the empty token", String::new()),
        ("a token another listing minted", minted_elsewhere.to_string()),
    ]
}

/// `GET <path>?nextToken=<token>`.
async fn listed(router: &MultiServiceRouter, path: &str, token: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("{path}?nextToken={}", encode_label(token)),
        "",
    )
    .await
}

/// The cluster, the access entry and the two policy associations every listing below
/// reads, and the path `ListAssociatedAccessPolicies` is addressed at.
async fn standing_resources(router: &MultiServiceRouter) -> String {
    active_cluster(router, "setup", CLUSTER).await;
    let principal = principal_arn("PlaneWordingReader");
    let entry = create_entry(router, CLUSTER, &principal).await;
    assert_eq!(
        entry.status, 200,
        "setup: expected 200 from CreateAccessEntry, got {entry:?}"
    );
    // Two associations, so the access-entry listing has a page to leave a token on.
    for policy in ["AmazonEKSAdminPolicy", "AmazonEKSEditPolicy"] {
        let granted =
            associate_policy(router, CLUSTER, &principal, &access_policy_arn(policy)).await;
        assert_eq!(
            granted.status, 200,
            "setup: expected 200 from AssociateAccessPolicy for {policy:?}, got {granted:?}"
        );
    }
    format!(
        "/clusters/{}/access-entries/{}/access-policies",
        encode_label(CLUSTER),
        encode_label(&principal)
    )
}

/// Every listing refuses a token it did not issue as a malformed parameter, in the
/// sentence ITS OWN plane states such a refusal in, echoing nothing of the request.
#[tokio::test]
async fn a_page_token_no_listing_minted_is_refused_in_its_own_planes_sentence() {
    let router = emulator();
    let policies_path = standing_resources(&router).await;

    // A token a listing of this service really minted, which every OTHER listing must
    // refuse: the access-entry listing's own first page leaves one.
    let minted = {
        let first = call(
            &router,
            REGION,
            "GET",
            &format!("{policies_path}?maxResults=1"),
            "",
        )
        .await;
        assert_eq!(
            first.status, 200,
            "setup: expected a served first page, got {first:?}"
        );
        first.body["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("setup: a one-policy page must leave a nextToken, got {first:?}")
            })
            .to_owned()
    };

    for Listing {
        operation,
        path,
        plane,
    } in listings(&policies_path)
    {
        for (row, token) in tokens(&minted) {
            // The token this very listing minted is a token OF it: it resumes the
            // listing rather than being refused by it.
            if row == "a token another listing minted" && path == policies_path {
                continue;
            }
            let case = format!("{operation} / {row}");
            let answer = listed(&router, &path, &token).await;
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (MALFORMED_PARAMETER_STATUS, MALFORMED_PARAMETER),
                "case {case:?}: a token no listing minted is a malformed parameter, got {answer:?}"
            );
            let reported = answer.message();
            assert_eq!(
                reported,
                plane.sentence(),
                "case {case:?}: expected the sentence the {plane:?} plane words this refusal in, \
                 got {reported:?}"
            );
            assert!(
                token.is_empty() || !reported.contains(token.as_str()),
                "case {case:?}: the refusal must echo nothing of the request, got {reported:?}"
            );
        }
    }
}

/// The two sentences are not interchangeable: ONE token, handed to a listing of each
/// plane, earns the sentence of that plane — the access-entry plane naming the member at
/// fault and what is wrong with it, the cluster-management plane Amazon EKS's own
/// page-token sentence.
#[tokio::test]
async fn the_two_planes_answer_the_same_token_in_different_sentences() {
    let router = emulator();
    let policies_path = standing_resources(&router).await;
    const TOKEN: &str = "deadbeefdeadbeefdeadbeef";

    let from_access_entry = listed(&router, &policies_path, TOKEN).await;
    let from_cluster_plane = listed(&router, "/clusters", TOKEN).await;
    let (access_entry, cluster_plane) = (from_access_entry.message(), from_cluster_plane.message());
    assert_ne!(
        access_entry, cluster_plane,
        "case one token, two planes: expected each plane's own sentence, got {access_entry:?} \
         from both"
    );
    // The access-entry plane's form: the member at fault, then what is wrong with it.
    assert_eq!(
        access_entry.strip_prefix("The specified nextToken is invalid: "),
        Some("unrecognized format."),
        "case the access-entry plane: expected the member and the fault, got {access_entry:?}"
    );
    // And the cluster-management plane's: one sentence covering a token it cannot read
    // and one whose position is gone, naming no reason the caller has to act on.
    assert_eq!(
        cluster_plane, "nextToken is not valid or expired.",
        "case the cluster-management plane: expected Amazon EKS's page-token sentence, got \
         {cluster_plane:?}"
    );
}

/// The boundary of the rule: it is about a token the listing did NOT issue. One it did
/// resumes the listing, so neither sentence is reachable by correct pagination — and an
/// untokened read is served throughout.
#[tokio::test]
async fn a_token_a_listing_issued_resumes_that_listing() {
    let router = emulator();
    let policies_path = standing_resources(&router).await;

    let first = call(
        &router,
        REGION,
        "GET",
        &format!("{policies_path}?maxResults=1"),
        "",
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case the first page: expected it to be served, got {first:?}"
    );
    let token = first.body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case the first page: expected a nextToken, got {first:?}"))
        .to_owned();
    let second = call(
        &router,
        REGION,
        "GET",
        &format!("{policies_path}?maxResults=1&nextToken={token}"),
        "",
    )
    .await;
    assert_eq!(
        second.status, 200,
        "case the resumed page: expected the listing's own token to resume it, got {second:?}"
    );
    let whole = call(&router, REGION, "GET", &policies_path, "").await;
    assert_eq!(
        whole.status, 200,
        "case the untokened read: expected it to be served, got {whole:?}"
    );
    let count = whole.body["associatedAccessPolicies"]
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        count, 2,
        "case the untokened read: expected both associations, got {whole:?}"
    );
}
