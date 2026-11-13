// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListAccessEntries`: the listing of one cluster's access
//! entries, driven over the wire through the same `MultiServiceRouter` the
//! production binary mounts.
//!
//! The write and the single read it pages over (`CreateAccessEntry`,
//! `DescribeAccessEntry`) are covered by `access_entries.rs`; this suite is about
//! what the LISTING adds: which entries it reports, in which order, how it pages
//! them, how it narrows them with `associatedPolicyArn`, and what it answers for a
//! cluster that does not exist.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (how many
//! entries a cluster holds, principal shape, region, page size, how a token is
//! foreign, which policy is filtered on), and each assertion names its case and
//! prints the actual value.
//!
//! The wire harness is the crate's one copy in `tests/all/wire.rs` and the
//! cluster fixture is `cluster_lifecycle`'s `minimal_request`: nothing is
//! duplicated here.

use std::collections::BTreeSet;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    access_policy_arn, cluster_not_found_message, encode_page_token, ACCESS_POLICY_NAMES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, Answer};

/// The emulator's own canonical account id, which every ARN it mints carries.
const ACCOUNT: &str = "000000000000";

fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

async fn create_cluster_in(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    let mut body = minimal_request(name);
    // This suite is about the entries `CreateAccessEntry` writes, so each cluster is
    // created WITHOUT the cluster creator's bootstrap entry
    // (`accessConfig.bootstrapClusterCreatorAdminPermissions`, which otherwise
    // defaults to true and files one for the calling principal — see
    // `cluster_creator_access_entry.rs`): what this lists is then exactly what it
    // created. The `authenticationMode` beside it is the one that serves the
    // access-entry API at all (see `access_entry_cluster_request`).
    body["accessConfig"] = serde_json::json!({
        "authenticationMode": "API_AND_CONFIG_MAP",
        "bootstrapClusterCreatorAdminPermissions": false,
    });
    call(router, region, "POST", "/clusters", &body.to_string()).await
}

async fn create_cluster(router: &MultiServiceRouter, name: &str) -> Answer {
    create_cluster_in(router, REGION, name).await
}

async fn create_entry_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    principal: &str,
) -> Answer {
    let body = serde_json::json!({ "principalArn": principal });
    call(
        router,
        region,
        "POST",
        &format!("/clusters/{}/access-entries", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn create_entry(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    create_entry_in(router, REGION, cluster, principal).await
}

async fn associate(
    router: &MultiServiceRouter,
    cluster: &str,
    principal: &str,
    policy_arn: &str,
) -> Answer {
    let body = serde_json::json!({
        "policyArn": policy_arn,
        "accessScope": { "type": "cluster" },
    });
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/access-entries/{}/access-policies",
            encode_label(cluster),
            encode_label(principal)
        ),
        &body.to_string(),
    )
    .await
}

async fn list_entries_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    query: &str,
) -> Answer {
    let path = format!("/clusters/{}/access-entries", encode_label(cluster));
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, region, "GET", &uri, "").await
}

async fn list_entries(router: &MultiServiceRouter, cluster: &str, query: &str) -> Answer {
    list_entries_in(router, REGION, cluster, query).await
}

/// The `accessEntries` of a listing answer, in the order it returned them.
fn listed(answer: &Answer) -> Vec<String> {
    answer.body["accessEntries"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// RULE: the listing reports exactly the access entries the store holds for the
// named cluster — every one that was created, nothing that was not — in sorted
// principal-ARN order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_reports_exactly_the_entries_created_for_that_cluster() {
    let router = emulator();

    // Rows differ along how many entries the cluster holds and along the shape of
    // the principals: an empty cluster, one entry, several created OUT of sorted
    // order, and an IAM user beside roles.
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        ("no entries at all", "empty-cluster", Vec::new()),
        ("a single entry", "one-entry", vec![role_arn("solo")]),
        (
            "several entries created out of sorted order",
            "many-entries",
            vec![role_arn("zeta"), role_arn("alpha"), role_arn("mu")],
        ),
        (
            "principal types that differ",
            "mixed-principals",
            vec![
                format!("arn:aws:iam::{ACCOUNT}:user/analyst"),
                role_arn("plain"),
            ],
        ),
    ];

    for (case, cluster, principals) in &cases {
        let created = create_cluster(&router, cluster).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup CreateCluster expected 200, got {created:?}"
        );
        for principal in principals {
            let answer = create_entry(&router, cluster, principal).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: CreateAccessEntry {principal:?} expected 200, got {answer:?}"
            );
        }

        let expected: Vec<String> = principals
            .iter()
            .cloned()
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect();
        let answer = list_entries(&router, cluster, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: ListAccessEntries expected 200, got {answer:?}"
        );
        assert_eq!(
            listed(&answer),
            expected,
            "case {case:?}: expected the stored principals in sorted order {expected:?}, \
             got {answer:?}"
        );
        assert!(
            answer.body["nextToken"].is_null(),
            "case {case:?}: a single page offers no nextToken, got {answer:?}"
        );
    }

    // No cluster's listing leaks another's entries, and a listing is a READ: the
    // second call reports exactly what the first did.
    for (case, cluster, principals) in &cases {
        let first = list_entries(&router, cluster, "").await;
        let again = list_entries(&router, cluster, "").await;
        assert_eq!(
            listed(&first).len(),
            principals.len(),
            "case {case:?}: a listing reports only its own cluster's entries, got {first:?}"
        );
        assert_eq!(
            listed(&again),
            listed(&first),
            "case {case:?}: a listing must not change what the next caller sees, got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster this store does not hold has no listing at all —
// `ResourceNotFoundException` (404), not an empty page.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_a_cluster_that_does_not_exist_is_not_found() {
    let router = emulator();
    let created = create_cluster(&router, "present").await;
    assert_eq!(created.status, 200, "setup: {created:?}");

    // Rows differ along WHY the cluster is not in the store for this request: it
    // was never created, its name differs in case, and it lives in another region.
    let cases: Vec<(&str, &str, Answer)> = vec![
        (
            "a cluster that was never created",
            "absent",
            list_entries(&router, "absent", "").await,
        ),
        (
            "a cluster name differing only in case",
            "PRESENT",
            list_entries(&router, "PRESENT", "").await,
        ),
        (
            "the right name in another region",
            "present",
            list_entries_in(&router, "eu-west-1", "present", "").await,
        ),
    ];
    for (case, cluster, answer) in &cases {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        // This listing resolves the CLUSTER and nothing else, so the cluster is the
        // resource it reports missing: the message names it, and `clusterName`
        // carries it. (The access-entry READS, which resolve the `(cluster, entry)`
        // pair, report the generic sentence with no member instead.)
        assert_eq!(
            answer.body["message"].as_str(),
            Some(cluster_not_found_message(cluster).as_str()),
            "case {case:?}: expected the message to name the missing cluster, got {answer:?}"
        );
        assert_eq!(
            answer.body["clusterName"].as_str(),
            Some(*cluster),
            "case {case:?}: expected clusterName to name the missing cluster, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster name identifies a cluster only within one region, so an access
// entry is listed only in the region its cluster was created in.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn entries_are_scoped_to_the_region_their_cluster_lives_in() {
    let router = emulator();
    let cluster = "shared-name";

    // Rows differ along the region the pair of calls is signed for.
    let cases = [
        ("us-west-2", role_arn("west")),
        ("eu-central-1", role_arn("central")),
        ("ap-southeast-1", role_arn("southeast")),
    ];

    for (region, principal) in &cases {
        let created = create_cluster_in(&router, region, cluster).await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: setup CreateCluster expected 200, got {created:?}"
        );
        let answer = create_entry_in(&router, region, cluster, principal).await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: CreateAccessEntry expected 200, got {answer:?}"
        );
    }

    for (region, principal) in &cases {
        let answer = list_entries_in(&router, region, cluster, "").await;
        assert_eq!(
            listed(&answer),
            vec![principal.clone()],
            "case {region:?}: a region lists only its own cluster's entries, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: whatever page size the listing is driven at, following `nextToken` to
// exhaustion yields each principal exactly once in sorted order and the last page
// carries no token; a token this listing did not issue is refused rather than
// resumed from.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_at_any_size_yields_every_entry_exactly_once() {
    let router = emulator();
    let cluster = "paged";
    assert_eq!(
        create_cluster(&router, cluster).await.status,
        200,
        "setup CreateCluster must succeed"
    );
    let stored: Vec<String> = (0..7).map(|index| role_arn(&format!("r{index}"))).collect();
    for principal in &stored {
        let answer = create_entry(&router, cluster, principal).await;
        assert_eq!(
            answer.status, 200,
            "setup CreateAccessEntry {principal:?}: got {answer:?}"
        );
    }

    // Rows differ along page size: the modelled bounds (1 and 100), the sizes that
    // divide and do not divide the 7 stored entries, and the exact count.
    for page_size in [1, 2, 3, 6, 7, 8, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(value) => format!("maxResults={page_size}&nextToken={value}"),
            };
            let answer = list_entries(&router, cluster, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size}: expected 200, got {answer:?}"
            );
            let page = listed(&answer);
            assert!(
                page.len() <= page_size as usize,
                "case maxResults={page_size}: a page must not exceed the limit, got {page:?}"
            );
            seen.extend(page.clone());
            pages += 1;
            assert!(
                pages <= stored.len() + 1,
                "case maxResults={page_size}: paging did not terminate after {pages} pages"
            );
            match answer.body["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        !page.is_empty(),
                        "case maxResults={page_size}: a token was returned for an empty page"
                    );
                    token = Some(next.to_owned());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, stored,
            "case maxResults={page_size}: paging must yield every principal once, in \
             sorted order; got {seen:?}"
        );
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case maxResults={page_size}: a principal was repeated across pages: {seen:?}"
        );
    }

    // The rows differ along HOW the token is foreign: not hex, an odd length, a
    // well-formed encoding carrying no provenance tag, and — the one a scope-less
    // token check would wave through — a token another listing of this same service
    // minted (`ListClusters`).
    let untagged: String = stored[0].bytes().map(|byte| format!("{byte:02x}")).collect();
    let other_listing = encode_page_token(cluster);
    for case in [
        "not-hex",
        "zz",
        "abc",
        "ffffffffff",
        untagged.as_str(),
        other_listing.as_str(),
    ] {
        let answer = list_entries(&router, cluster, &format!("nextToken={case}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a foreign nextToken, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `associatedPolicyArn` narrows the listing to the entries that access
// policy is associated to — "only the access entries associated to that access
// policy are returned" — and leaves the unfiltered listing reporting them all.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_policy_filtered_listing_reports_only_the_associated_entries() {
    let router = emulator();
    let cluster = "filtered";
    assert_eq!(
        create_cluster(&router, cluster).await.status,
        200,
        "setup CreateCluster must succeed"
    );

    // Three entries: one associated to the FIRST published policy, one to the
    // SECOND, one to neither — so a filter must select a strict subset, not "all"
    // and not "none".
    let first_policy = access_policy_arn(ACCESS_POLICY_NAMES[0]);
    let second_policy = access_policy_arn(ACCESS_POLICY_NAMES[1]);
    let on_first = role_arn("on-first-policy");
    let on_second = role_arn("on-second-policy");
    let unassociated = role_arn("unassociated");
    let mut stored = vec![on_first.clone(), on_second.clone(), unassociated.clone()];
    stored.sort();
    for principal in &stored {
        let answer = create_entry(&router, cluster, principal).await;
        assert_eq!(
            answer.status, 200,
            "setup CreateAccessEntry {principal:?}: got {answer:?}"
        );
    }
    for (principal, policy) in [(&on_first, &first_policy), (&on_second, &second_policy)] {
        let answer = associate(&router, cluster, principal, policy).await;
        assert_eq!(
            answer.status, 200,
            "setup AssociateAccessPolicy {policy:?} on {principal:?}: got {answer:?}"
        );
    }

    let unfiltered = list_entries(&router, cluster, "").await;
    assert_eq!(
        listed(&unfiltered),
        stored,
        "the unfiltered listing reports every entry, got {unfiltered:?}"
    );

    // Rows differ along which policy is filtered on: each of the two associated
    // ones, a published policy nothing is associated to, and a well-formed ARN of
    // no published policy at all.
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "the first associated policy",
            first_policy.clone(),
            vec![on_first.clone()],
        ),
        (
            "the second associated policy",
            second_policy.clone(),
            vec![on_second.clone()],
        ),
        (
            "a published policy no entry is associated to",
            access_policy_arn(ACCESS_POLICY_NAMES[2]),
            Vec::new(),
        ),
        (
            "an ARN naming no published policy",
            "arn:aws:eks::aws:cluster-access-policy/AmazonEKSNoSuchPolicy".to_string(),
            Vec::new(),
        ),
    ];
    for (case, policy, expected) in &cases {
        let answer = list_entries(
            &router,
            cluster,
            &format!("associatedPolicyArn={}", encode_label(policy)),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        assert_eq!(
            &listed(&answer),
            expected,
            "case {case:?}: expected exactly the entries that policy is associated to \
             {expected:?}, got {answer:?}"
        );
        if expected.is_empty() {
            assert!(
                answer.body["nextToken"].is_null(),
                "case {case:?}: an empty page offers no nextToken, got {answer:?}"
            );
        }
    }

    // The filter pages like the unfiltered listing: driven at one result per page
    // over a policy two entries share, it still yields each exactly once.
    let answer = associate(&router, cluster, &unassociated, &first_policy).await;
    assert_eq!(answer.status, 200, "setup: a second entry on the first policy: {answer:?}");
    let mut expected = vec![on_first.clone(), unassociated.clone()];
    expected.sort();
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            None => format!(
                "maxResults=1&associatedPolicyArn={}",
                encode_label(&first_policy)
            ),
            Some(value) => format!(
                "maxResults=1&nextToken={value}&associatedPolicyArn={}",
                encode_label(&first_policy)
            ),
        };
        let answer = list_entries(&router, cluster, &query).await;
        assert_eq!(
            answer.status, 200,
            "case a filtered page: expected 200, got {answer:?}"
        );
        seen.extend(listed(&answer));
        assert!(
            seen.len() <= expected.len(),
            "case a filtered page: paging reported more than the filter selects: {seen:?}"
        );
        match answer.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_owned()),
            None => break,
        }
    }
    assert_eq!(
        seen, expected,
        "a filtered listing pages over exactly the associated entries, got {seen:?}"
    );
}
