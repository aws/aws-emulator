// SPDX-License-Identifier: CC0-1.0

//! The CLASS and the SENTENCE of three refusals this service composes itself,
//! driven end to end over the same `MultiServiceRouter` the production binary
//! mounts.
//!
//! An error's code and its message are both part of its contract — a client
//! branches on the code and matches the sentence — so each suite below asserts the
//! whole sentence rather than a substring, over a TABLE whose rows differ along what
//! the rule generalises over:
//!
//! * AN UPDATE ASKED FOR BY AN ID THAT RESOLVES TO NOTHING is
//!   `ResourceNotFoundException` (404) stating the UPDATE by the id the request
//!   carried, in one sentence whichever operation asked and whichever way the lookup
//!   missed (never minted, the wrong case, or an id of this cluster held under
//!   another scope). Rows differ along the shape of the id, along the operation, and
//!   along the member that scoped the lookup.
//! * DELETING A CLUSTER WHOSE DELETION PROTECTION IS ON is `InvalidRequestException`
//!   (400) naming the cluster and the switch that forbids the call, and the cluster
//!   is left exactly where it was. Rows differ along the shape of the cluster name
//!   and along how the switch came to be on (set by the create, or turned on
//!   afterwards), and the switch — not the name — is what decides: turned off again,
//!   the same cluster deletes.
//! * A `nextToken` NO LISTING MINTED is `InvalidParameterException` (400) — a
//!   malformed parameter, not the state-dependent `InvalidRequestException` — with
//!   this service's one page-token sentence, on EVERY paginated listing it serves.
//!   Rows differ along the listing, along where the token travels (the query string
//!   or the request body) and along how the token misses.
//!
//! Each suite also checks the helper the handlers compose the sentence with against
//! the text written out here, so neither the handlers nor the shared helper can drift
//! from it while this suite is green, and each carries a CONTROL row that is SERVED,
//! so a refusal is never mistaken for an operation that refuses everything.
//!
//! Nothing here waits for a fixed span: where the clock owes a cluster a move, the
//! read is repeated until the condition holds or a generous deadline passes. That is
//! why the cluster precondition is a local helper rather than one of the other
//! suites' same-named ones: those wait out the span instead of the condition. The
//! shared harness this suite does reuse is the crate's one copy of the wire plumbing
//! (`tests/all/wire.rs`) and the cluster request body and paths
//! (`tests/all/cluster_lifecycle.rs`); the per-operation paths each suite in this
//! crate keeps are private to it, so the few this one needs are below.

use std::time::{Duration, Instant};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    access_policy_arn, cluster_deletion_protected, update_not_found_message,
    INVALID_NEXT_TOKEN_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{
    access_entry_cluster_request, create, delete, describe, minimal_request, REGION,
};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// How long a clock-owed cluster move is given before the suite calls it a failure.
/// Generous on purpose: what is asserted is the state a call reaches, never how fast
/// the host gets there.
const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// How often the poll loops look again. See [`POLL_DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The status a cluster carries once the clock has finished creating it.
const ACTIVE: &str = "ACTIVE";

/// An IAM principal an access entry can be created for.
fn principal_arn(name: &str) -> String {
    format!("arn:aws:iam::000000000000:role/{name}")
}

/// `UpdateClusterConfig`: `POST /clusters/{name}/update-config`.
async fn update_config(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/update-config", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

/// `CreateAccessEntry`: `POST /clusters/{clusterName}/access-entries`.
async fn create_entry(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/access-entries", encode_label(cluster)),
        &serde_json::json!({ "principalArn": principal }).to_string(),
    )
    .await
}

/// `AssociateAccessPolicy`: grants one published policy to one access entry.
async fn associate_policy(
    router: &MultiServiceRouter,
    cluster: &str,
    principal: &str,
    policy_arn: &str,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/access-entries/{}/access-policies",
            encode_label(cluster),
            encode_label(principal)
        ),
        &serde_json::json!({
            "policyArn": policy_arn,
            "accessScope": { "type": "cluster" },
        })
        .to_string(),
    )
    .await
}

/// Read `DescribeCluster` until `settled` holds of the cluster it reports, or the
/// deadline passes — the condition is waited for, never a span.
async fn cluster_until<F>(
    router: &MultiServiceRouter,
    case: &str,
    what: &str,
    cluster: &str,
    settled: F,
) -> Answer
where
    F: Fn(&serde_json::Value) -> bool,
{
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let answer = describe(router, cluster).await;
        if answer.status == 200 && settled(answer.cluster()) {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the cluster never reached {what}, last read {answer:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A created cluster, read back once the clock has carried it to `ACTIVE` — the
/// precondition an update or an access entry needs.
///
/// The body is the one whose `authenticationMode` serves the access-entry API, since
/// that is one of the preconditions this provides.
async fn active_cluster(router: &MultiServiceRouter, case: &str, cluster: &str) {
    let created = create(router, &access_entry_cluster_request(cluster)).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: setup CreateCluster expected 200, got {created:?}"
    );
    cluster_until(router, case, ACTIVE, cluster, |cluster| {
        cluster["status"] == serde_json::json!(ACTIVE)
    })
    .await;
}

// ---------------------------------------------------------------------------
// RULE: an `updateId` that resolves to no update of the resource the request
// addresses is 404 ResourceNotFoundException stating the update by that id, in
// ONE sentence — whichever operation asked, and whichever way the lookup
// missed.
// ---------------------------------------------------------------------------

/// Which operation asks for the update, and under which scoping member. The scope is
/// the query string `DescribeUpdate` narrows the lookup with — a resource inside the
/// cluster whose updates are the only ones that may answer.
#[derive(Debug, Clone, Copy)]
enum Caller {
    CancelUpdate,
    /// `DescribeUpdate`, with this query string (empty for the cluster's own plane).
    DescribeUpdate(&'static str),
}

async fn ask_for_update(
    router: &MultiServiceRouter,
    caller: Caller,
    cluster: &str,
    update_id: &str,
) -> Answer {
    let addressed = format!(
        "/clusters/{}/updates/{}",
        encode_label(cluster),
        encode_label(update_id)
    );
    match caller {
        Caller::CancelUpdate => {
            call(
                router,
                REGION,
                "POST",
                &format!("{addressed}/cancel-update"),
                "{}",
            )
            .await
        }
        Caller::DescribeUpdate(scope) => {
            let uri = if scope.is_empty() {
                addressed
            } else {
                format!("{addressed}?{scope}")
            };
            call(router, REGION, "GET", &uri, "").await
        }
    }
}

#[tokio::test]
async fn an_update_id_that_resolves_to_nothing_is_refused_in_one_fixed_sentence() {
    const CLUSTER: &str = "update-sentence";
    let router = emulator();
    active_cluster(&router, "setup", CLUSTER).await;

    // A REAL update of this cluster's own control plane, so one row can ask for an id
    // that EXISTS under a scope that does not hold it — the third way the lookup can
    // miss, beside an id never minted and one in the wrong case.
    let minted = update_config(
        &router,
        CLUSTER,
        &serde_json::json!({
            "logging": { "clusterLogging": [{ "types": ["api"], "enabled": true }] },
        }),
    )
    .await;
    assert_eq!(
        minted.status, 200,
        "setup UpdateClusterConfig expected 200, got {minted:?}"
    );
    let real_id = minted.body["update"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the update must report its id, got {minted:?}"))
        .to_owned();

    // CONTROL: that id IS served unscoped, so the refusals below are about the id and
    // the scope the row names, not about an operation that refuses everything.
    let served = ask_for_update(&router, Caller::DescribeUpdate(""), CLUSTER, &real_id).await;
    assert_eq!(
        served.status, 200,
        "control: the minted update id is readable, got {served:?}"
    );

    // Rows differ along the SHAPE of the id that resolves to nothing: a well-formed
    // UUID nothing minted, the minted id in the wrong case (ids are case-sensitive),
    // a value that is no UUID at all, one carrying characters path encoding must
    // carry through, an empty-looking single character, and a very long one.
    let long_id = "9".repeat(256);
    let unresolvable: Vec<(&str, String)> = vec![
        (
            "a well-formed UUID nothing minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("the minted id in upper case", real_id.to_uppercase()),
        ("a value that is not a UUID", "not-an-update-id".to_string()),
        (
            "a value needing path encoding",
            "update id/with slash".to_string(),
        ),
        ("a single character", "x".to_string()),
        ("a 256-character value", long_id),
    ];

    // The callers: the cancel path, the read of the cluster's own plane, and the read
    // narrowed by each of the three scoping members the model declares
    // (`smithy_query query="operation" operation="DescribeUpdate"`:
    // `nodegroupName`, `addonName`, `capabilityName`).
    let callers = [
        ("CancelUpdate", Caller::CancelUpdate),
        ("DescribeUpdate", Caller::DescribeUpdate("")),
        (
            "DescribeUpdate scoped by nodegroupName",
            Caller::DescribeUpdate("nodegroupName=workers"),
        ),
        (
            "DescribeUpdate scoped by addonName",
            Caller::DescribeUpdate("addonName=vpc-cni"),
        ),
        (
            "DescribeUpdate scoped by capabilityName",
            Caller::DescribeUpdate("capabilityName=ACK"),
        ),
    ];

    for (row, update_id) in &unresolvable {
        for (operation, caller) in callers {
            let case = format!("{row} / {operation}");
            let answer = ask_for_update(&router, caller, CLUSTER, update_id).await;
            assert_refused_as_missing_update(&case, &answer, update_id, CLUSTER);
        }
    }

    // And the id that DOES exist, asked for under each scope that does not hold it:
    // the update belongs to the cluster's own control plane, so a lookup narrowed to
    // a resource inside the cluster reports the same sentence as an id nothing minted.
    for (operation, caller) in callers
        .into_iter()
        .filter(|(_, caller)| !matches!(caller, Caller::DescribeUpdate("")))
        .filter(|(_, caller)| !matches!(caller, Caller::CancelUpdate))
    {
        let case = format!("an existing cluster-plane id / {operation}");
        let answer = ask_for_update(&router, caller, CLUSTER, &real_id).await;
        assert_refused_as_missing_update(&case, &answer, &real_id, CLUSTER);
    }
}

/// What the rule requires of one refusal: the 404 class, the modelled code, and the
/// one sentence — AWS's own fixed text around the id the request carried, written out
/// here rather than only imported, with the shared helper then checked against it so
/// neither it nor a handler can drift from this text.
fn assert_refused_as_missing_update(
    case: &str,
    answer: &Answer,
    update_id: &str,
    cluster: &str,
) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
    );
    let actual = answer.message();
    let expected = format!("Update {update_id} not found");
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the update-not-found sentence, got {actual:?}"
    );
    assert_eq!(
        update_not_found_message(update_id),
        expected,
        "case {case:?}: the helper every caller words this refusal with must be AWS's \
         sentence, got {:?}",
        update_not_found_message(update_id)
    );
    assert!(
        !actual.contains(cluster),
        "case {case:?}: the refusal reports the UPDATE, so it must not name the \
         cluster, got {actual:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: deleting a cluster whose DeletionProtection is on is 400
// InvalidRequestException naming the cluster and the switch, and the cluster is
// left where it was; the switch, not the name, is what decides.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_protected_cluster_is_refused_naming_the_cluster_and_the_switch() {
    let router = emulator();

    // Rows differ along the shape of the cluster name — the one variable part of the
    // sentence — over the forms the modelled `ClusterName` pattern allows
    // (`^[0-9A-Za-z][A-Za-z0-9\-_]*$`, length 1..100: `smithy_query query="shape"
    // shape="ClusterName"`): one character, mixed case with digits and both
    // separators, and the published maximum length.
    let max_length_name = format!("p{}", "9".repeat(99));
    let protected_at_create: Vec<(&str, String)> = vec![
        ("a one-character name", "p".to_string()),
        (
            "a name carrying both separators, digits and mixed case",
            "Prot_ected-9".to_string(),
        ),
        (
            "a 100-character name, the published maximum",
            max_length_name,
        ),
    ];

    for (case, name) in &protected_at_create {
        let mut request = minimal_request(name);
        request["deletionProtection"] = serde_json::json!(true);
        let created = create(&router, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup CreateCluster expected 200, got {created:?}"
        );
        assert_protected_against_deletion(&router, case, name).await;
    }

    // The same rule for a cluster created UNPROTECTED whose switch is turned on
    // afterwards: how the switch came to be on is not part of the rule. The read is
    // repeated until the update the clock owes has applied it.
    const TURNED_ON: &str = "turned-on-later";
    let case = "protection turned on by UpdateClusterConfig";
    active_cluster(&router, case, TURNED_ON).await;
    let turned_on = update_config(
        &router,
        TURNED_ON,
        &serde_json::json!({ "deletionProtection": true }),
    )
    .await;
    assert_eq!(
        turned_on.status, 200,
        "case {case:?}: setup UpdateClusterConfig expected 200, got {turned_on:?}"
    );
    cluster_until(&router, case, "deletionProtection on", TURNED_ON, |cluster| {
        cluster["deletionProtection"] == serde_json::json!(true)
    })
    .await;
    assert_protected_against_deletion(&router, case, TURNED_ON).await;

    // And turned OFF again the same cluster deletes: the refusal above is the state of
    // the switch, not anything about the cluster or its name.
    let case = "protection turned off again";
    let turned_off = update_config(
        &router,
        TURNED_ON,
        &serde_json::json!({ "deletionProtection": false }),
    )
    .await;
    assert_eq!(
        turned_off.status, 200,
        "case {case:?}: UpdateClusterConfig expected 200, got {turned_off:?}"
    );
    cluster_until(&router, case, "deletionProtection off", TURNED_ON, |cluster| {
        cluster["deletionProtection"] == serde_json::json!(false)
    })
    .await;
    let removed = delete(&router, TURNED_ON).await;
    assert_eq!(
        removed.status, 200,
        "case {case:?}: an unprotected cluster deletes, got {removed:?}"
    );

    // CONTROL: a cluster that never asked for protection deletes too, so the rows
    // above are refused for their switch and not because `DeleteCluster` refuses
    // everything.
    const PLAIN: &str = "never-protected";
    let created = create(&router, &minimal_request(PLAIN)).await;
    assert_eq!(
        created.status, 200,
        "control: setup CreateCluster expected 200, got {created:?}"
    );
    let removed = delete(&router, PLAIN).await;
    assert_eq!(
        removed.status, 200,
        "control: an unprotected cluster deletes, got {removed:?}"
    );
}

/// `DeleteCluster` is refused for `cluster`, in the sentence the switch earns, and
/// the cluster is still there afterwards.
async fn assert_protected_against_deletion(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
) {
    let answer = delete(router, cluster).await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got {answer:?}"
    );
    let actual = answer.message();
    // AWS's own sentence, written out here rather than only imported, so a change to
    // the service's helper cannot keep this green; the helper is then checked against
    // the same text, which is what ties the handler to it.
    let expected =
        format!("Cluster {cluster} cannot be deleted while DeletionProtection is enabled.");
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the deletion-protection sentence, got {actual:?}"
    );
    assert_eq!(
        cluster_deletion_protected(cluster),
        expected,
        "case {case:?}: the helper this refusal is worded with must be AWS's sentence, \
         got {:?}",
        cluster_deletion_protected(cluster)
    );

    // Nothing was torn down: the refused delete left the cluster readable.
    let read = describe(router, cluster).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: a refused delete must leave the cluster readable, got {read:?}"
    );
    assert_eq!(
        read.cluster()["deletionProtection"],
        serde_json::json!(true),
        "case {case:?}: the switch must still be on, got {:?}",
        read.cluster()["deletionProtection"]
    );
}

// ---------------------------------------------------------------------------
// RULE: a `nextToken` no listing minted is a MALFORMED PARAMETER — 400
// InvalidParameterException with this service's one page-token sentence — on
// every paginated listing it serves, and never the state-dependent
// InvalidRequestException.
// ---------------------------------------------------------------------------

/// Where a listing's `nextToken` travels: the query string for the listings the model
/// binds it to one with, the request body for the one it does not (`ListInsights`).
#[derive(Debug, Clone, Copy)]
enum Transport {
    Query,
    Body,
}

#[tokio::test]
async fn a_page_token_no_listing_minted_is_a_malformed_parameter_on_every_listing() {
    const CLUSTER: &str = "token-cluster";
    const SECOND: &str = "token-cluster-two";
    let router = emulator();
    active_cluster(&router, "setup", CLUSTER).await;

    // A second cluster, so `ListClusters` has a page to leave a token on — the token
    // one listing minted is one of the rows every OTHER listing must refuse.
    let created = create(&router, &minimal_request(SECOND)).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster for {SECOND:?}, got {created:?}"
    );

    // An access entry holding two published policies, so `ListAssociatedAccessPolicies`
    // — the listing this rule was filed against — has a page to resume as well.
    let principal = principal_arn("TokenReader");
    let entry = create_entry(&router, CLUSTER, &principal).await;
    assert_eq!(
        entry.status, 200,
        "setup: expected 200 from CreateAccessEntry, got {entry:?}"
    );
    for policy in ["AmazonEKSAdminPolicy", "AmazonEKSEditPolicy"] {
        let granted =
            associate_policy(&router, CLUSTER, &principal, &access_policy_arn(policy)).await;
        assert_eq!(
            granted.status, 200,
            "setup: expected 200 from AssociateAccessPolicy for {policy:?}, got {granted:?}"
        );
    }

    let policies_path = format!(
        "/clusters/{}/access-entries/{}/access-policies",
        encode_label(CLUSTER),
        encode_label(&principal)
    );

    // The token `ListClusters` really minted, and the same token with one hex digit
    // changed: a well-formed token of ANOTHER listing, and a token of no listing at
    // all.
    let minted = call(&router, REGION, "GET", "/clusters?maxResults=1", "").await;
    let clusters_token = minted.body["nextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("setup: a one-cluster page must leave a nextToken, got {minted:?}")
        })
        .to_owned();
    let corrupted = {
        let mut token = clusters_token.clone();
        let last = token.pop().expect("setup: the token is not empty");
        token.push(if last == '0' { '1' } else { '0' });
        token
    };

    // CONTROL: the token ITS OWN listing minted resumes that listing, so the rows
    // below are refused for the token they carry and not because pagination is broken.
    let resumed = call(
        &router,
        REGION,
        "GET",
        &format!("/clusters?maxResults=1&nextToken={clusters_token}"),
        "",
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "control: a token ListClusters minted resumes it, got {resumed:?}"
    );
    let policies_page = call(
        &router,
        REGION,
        "GET",
        &format!("{policies_path}?maxResults=1"),
        "",
    )
    .await;
    assert_eq!(
        policies_page.status, 200,
        "control: the associated-policy listing serves a page, got {policies_page:?}"
    );
    let policies_token = policies_page.body["nextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("setup: a one-policy page must leave a nextToken, got {policies_page:?}")
        })
        .to_owned();
    let resumed = call(
        &router,
        REGION,
        "GET",
        &format!("{policies_path}?maxResults=1&nextToken={policies_token}"),
        "",
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "control: a token ListAssociatedAccessPolicies minted resumes it, got {resumed:?}"
    );

    // Every paginated listing this service serves, with the path each is addressed at
    // and where its `nextToken` travels.
    let listings: Vec<(&str, String, Transport)> = vec![
        ("ListClusters", "/clusters".to_string(), Transport::Query),
        (
            "DescribeClusterVersions",
            "/cluster-versions".to_string(),
            Transport::Query,
        ),
        (
            "ListAccessPolicies",
            "/access-policies".to_string(),
            Transport::Query,
        ),
        (
            "ListEksAnywhereSubscriptions",
            "/eks-anywhere-subscriptions".to_string(),
            Transport::Query,
        ),
        (
            "ListAddons",
            format!("/clusters/{CLUSTER}/addons"),
            Transport::Query,
        ),
        (
            "ListNodegroups",
            format!("/clusters/{CLUSTER}/node-groups"),
            Transport::Query,
        ),
        (
            "ListUpdates",
            format!("/clusters/{CLUSTER}/updates"),
            Transport::Query,
        ),
        (
            "ListCapabilities",
            format!("/clusters/{CLUSTER}/capabilities"),
            Transport::Query,
        ),
        (
            "ListCertificateAuthorities",
            format!("/clusters/{CLUSTER}/certificate-authorities"),
            Transport::Query,
        ),
        (
            "ListAccessEntries",
            format!("/clusters/{CLUSTER}/access-entries"),
            Transport::Query,
        ),
        (
            "ListPodIdentityAssociations",
            format!("/clusters/{CLUSTER}/pod-identity-associations"),
            Transport::Query,
        ),
        (
            "ListAssociatedAccessPolicies",
            policies_path.clone(),
            Transport::Query,
        ),
        (
            "ListInsights",
            format!("/clusters/{CLUSTER}/insights"),
            Transport::Body,
        ),
    ];

    // Rows differ along HOW the token misses: a word that is not even hexadecimal,
    // hexadecimal bytes nothing minted, an empty token, a well-formed token another
    // listing minted, and that token with one digit changed. An empty `nextToken` is
    // a token the request SUPPLIED — the member is present, and no listing mints the
    // empty string — so it is read as one this service does not recognise rather than
    // as an absent member; absence is the untokened first page, which every control
    // above serves.
    let tokens: Vec<(&str, String)> = vec![
        ("a word", "not-a-token".to_string()),
        (
            "hexadecimal bytes nothing minted",
            "deadbeefdeadbeefdeadbeef".to_string(),
        ),
        ("an empty token", String::new()),
        ("another listing's token", clusters_token.clone()),
        ("a minted token with one digit changed", corrupted.clone()),
    ];

    for (operation, path, transport) in &listings {
        for (row, token) in &tokens {
            // The token `ListClusters` minted is a token of THAT listing: it resumes
            // it (asserted above) rather than being refused by it.
            if *row == "another listing's token" && *operation == "ListClusters" {
                continue;
            }
            let case = format!("{operation} / {row}");
            let answer = match transport {
                Transport::Query => {
                    call(
                        &router,
                        REGION,
                        "GET",
                        &format!("{path}?nextToken={}", encode_label(token)),
                        "",
                    )
                    .await
                }
                Transport::Body => {
                    call(
                        &router,
                        REGION,
                        "POST",
                        path,
                        &serde_json::json!({ "nextToken": token }).to_string(),
                    )
                    .await
                }
            };
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "InvalidParameterException",
                "case {case:?}: a token no listing minted is a malformed parameter, got \
                 {answer:?}"
            );
            let actual = answer.message();
            // AWS's own sentence, written out here rather than only imported, so a
            // change to the service's constant cannot keep this green; the constant
            // is then checked against the same text once below, which is what ties
            // every listing to it.
            assert_eq!(
                actual, "nextToken is not valid or expired.",
                "case {case:?}: expected this service's page-token sentence, got {actual:?}"
            );
            assert!(
                !actual.contains(token.as_str()) || token.is_empty(),
                "case {case:?}: the refusal must not echo the token, got {actual:?}"
            );
        }
    }

    assert_eq!(
        INVALID_NEXT_TOKEN_MESSAGE, "nextToken is not valid or expired.",
        "the constant every listing words this refusal with must be AWS's sentence, got \
         {INVALID_NEXT_TOKEN_MESSAGE:?}"
    );
}
