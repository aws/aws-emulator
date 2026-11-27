// SPDX-License-Identifier: CC0-1.0

//! `CreateVirtualCluster`, a SPENT `clientToken`, and the cluster that token already
//! names, end to end.
//!
//! Every call here goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` (the harness [`crate::virtual_cluster_job_run_loop`] owns) and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rule under test: a `clientToken` names ONE request, so a second call carrying a
//! token already spent in the same scope is a replay — answered with the resource the
//! first call minted — only when it carries the SAME request; one differing in ANY member
//! that reaches the stored resource is refused with the modelled `ValidationException`
//! (HTTP 400), persists nothing, and leaves the first resource exactly as its own request
//! described it.
//!
//! The suite beside this one ([`crate::spent_token_with_a_different_request`]) asserts the
//! refusal over four operations; this one asserts what the refused call must NOT have done
//! to the cluster the token names, at the depth a reader sees it — the whole
//! `DescribeVirtualCluster` answer, before and after — over a TABLE whose rows differ in
//! WHICH member of `CreateVirtualClusterRequest` the second call changed: a top-level
//! string, a member two and three levels down, a reference to another stored resource, a
//! boolean, a number at each end of its published range, a map's value, its entry count,
//! and a member dropped altogether.

use serde_json::{json, Value};

use crate::refusal_wording_by_error_class::SPENT_TOKEN_CONFLICT;
use crate::virtual_cluster_job_run_loop::{field, text, Emr};

/// Create one security configuration and answer its id, so a request can carry a
/// `securityConfigurationId` that really resolves: an id naming no stored configuration is
/// refused as a dangling reference, which would hide the token rule under test.
async fn security_configuration(emr: &Emr, name: &str, token: &str) -> String {
    let answer = emr
        .send(
            "POST",
            "/securityconfigurations",
            Some(json!({
                "name": name,
                "clientToken": token,
                // `securityConfigurationData` is `@required` (smithy_query: shape
                // `CreateSecurityConfigurationRequest`).
                "securityConfigurationData": {
                    "authorizationConfiguration": {
                        "lakeFormationConfiguration": {
                            "authorizedSessionTagValue": "Emr Session Tag",
                        },
                    },
                },
            })),
        )
        .await;
    text(answer.ok(name), "/id", name)
}

/// One acceptable `CreateVirtualCluster` request carrying EVERY member the model gives it
/// (smithy_query: shape `CreateVirtualClusterRequest`), so each row below can vary one of
/// them — and drop one — without the base request having to change.
fn base_request(security_configuration_id: &str) -> Value {
    json!({
        "name": "cluster.first",
        "clientToken": "vc-tok-parameters",
        "containerProvider": {
            "id": "eks-first",
            "type": "EKS",
            "info": {"eksInfo": {"namespace": "emr-first", "nodeLabel": "spot"}},
        },
        "securityConfigurationId": security_configuration_id,
        "sessionEnabled": true,
        // `maxConcurrentJobRuns` is `@range(1..1000)` and `maxInQueueJobRuns`
        // `@range(1..1000)` (smithy_query: shape `SchedulerConfiguration`).
        "schedulerConfiguration": {"maxConcurrentJobRuns": 4, "maxInQueueJobRuns": 500},
        "tags": {"team": "data", "env": "test"},
    })
}

/// Drop `member` from the object at `pointer` (`""` for the request itself): a row that
/// OMITS a member the first request carried is a different request too.
fn drop_member(body: &mut Value, pointer: &str, member: &str, case: &str) {
    let Some(object) = body.pointer_mut(pointer).and_then(Value::as_object_mut) else {
        panic!("case {case:?}: {pointer:?} names no object in this request");
    };
    assert!(
        object.remove(member).is_some(),
        "case {case:?}: the base request carries no {member:?} at {pointer:?} to drop"
    );
}

/// The ids `ListVirtualClusters` reports for this region.
async fn listed_ids(emr: &Emr, case: &str) -> Vec<String> {
    let answer = emr.send("GET", "/virtualclusters", None).await;
    let body = answer.ok(case);
    field(body, "/virtualClusters", case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: virtualClusters is not a list: {body}"))
        .iter()
        .map(|cluster| text(cluster, "/id", case))
        .collect()
}

/// The whole `DescribeVirtualCluster` answer for `id`.
async fn described(emr: &Emr, id: &str, case: &str) -> Value {
    let answer = emr.describe_cluster(id).await;
    answer.ok(case).clone()
}

/// Every member `CreateVirtualClusterRequest` and the `VirtualCluster` it mints share
/// (smithy_query: shapes `CreateVirtualClusterRequest`, `VirtualCluster`): what a describe
/// reports for each has to be what the request that MINTED the cluster spelled, never what
/// a later call carrying the same token asked for.
const MINTED_FROM_THE_REQUEST: [&str; 6] = [
    "name",
    "containerProvider",
    "securityConfigurationId",
    "sessionEnabled",
    "schedulerConfiguration",
    "tags",
];

/// Assert the described cluster reports the request that minted it, member by member.
fn assert_describes_the_minting_request(describe: &Value, request: &Value, case: &str) {
    let cluster = field(describe, "/virtualCluster", case);
    for member in MINTED_FROM_THE_REQUEST {
        assert_eq!(
            cluster.get(member),
            request.get(member),
            "case {case:?}: {member} must be the minting request's {:?}, got {:?}",
            request.get(member),
            cluster.get(member)
        );
    }
}

/// One row: how the second call's request differs from the first, given the id of a
/// SECOND stored security configuration it can point at.
struct Row {
    case: &'static str,
    differ: fn(&mut Value, &str, &str),
}

/// The rows: one per member of `CreateVirtualClusterRequest` that reaches the stored
/// cluster, plus the forms a member can differ in — a changed value, a value at the edge
/// of its published range, an added map entry, a removed one, and a dropped member.
const ROWS: [Row; 15] = [
    Row {
        case: "a different cluster name",
        differ: |body, _, _| body["name"] = json!("cluster.second"),
    },
    Row {
        case: "a different container provider id",
        differ: |body, _, _| body["containerProvider"]["id"] = json!("eks-second"),
    },
    Row {
        case: "a different Kubernetes namespace, three levels down",
        differ: |body, _, _| {
            body["containerProvider"]["info"]["eksInfo"]["namespace"] = json!("emr-second")
        },
    },
    Row {
        case: "a different node label, three levels down",
        differ: |body, _, _| {
            body["containerProvider"]["info"]["eksInfo"]["nodeLabel"] = json!("ondemand")
        },
    },
    Row {
        case: "the container info dropped",
        differ: |body, _, case| drop_member(body, "/containerProvider", "info", case),
    },
    Row {
        case: "another stored security configuration",
        differ: |body, other_configuration, _| {
            body["securityConfigurationId"] = json!(other_configuration)
        },
    },
    Row {
        case: "the securityConfigurationId dropped",
        differ: |body, _, case| drop_member(body, "", "securityConfigurationId", case),
    },
    Row {
        case: "sessionEnabled turned off",
        differ: |body, _, _| body["sessionEnabled"] = json!(false),
    },
    Row {
        case: "the concurrency limit at the top of its range",
        differ: |body, _, _| body["schedulerConfiguration"]["maxConcurrentJobRuns"] = json!(1000),
    },
    Row {
        case: "the concurrency limit at the bottom of its range",
        differ: |body, _, _| body["schedulerConfiguration"]["maxConcurrentJobRuns"] = json!(1),
    },
    Row {
        case: "the queue limit at the bottom of its range",
        differ: |body, _, _| body["schedulerConfiguration"]["maxInQueueJobRuns"] = json!(1),
    },
    Row {
        case: "the schedulerConfiguration dropped",
        differ: |body, _, case| drop_member(body, "", "schedulerConfiguration", case),
    },
    Row {
        case: "a different tag value",
        differ: |body, _, _| body["tags"]["team"] = json!("platform"),
    },
    Row {
        case: "one more tag",
        differ: |body, _, _| body["tags"]["owner"] = json!("analytics"),
    },
    Row {
        case: "one fewer tag",
        differ: |body, _, case| drop_member(body, "/tags", "env", case),
    },
];

/// A token already spent on `CreateVirtualCluster` in a region, carrying a request that
/// differs in ANY member, is refused — and the cluster it already names is left exactly as
/// the first request described it, while the first request REPLAYED still answers with it.
#[tokio::test]
async fn a_spent_token_never_rewrites_the_cluster_it_already_names() {
    for row in ROWS {
        let case = row.case;
        let emr = Emr::new();
        let configuration = security_configuration(&emr, "config.minting", "sc-tok-first").await;
        let other = security_configuration(&emr, "config.other", "sc-tok-other").await;

        let base = base_request(&configuration);
        let first = emr.send("POST", "/virtualclusters", Some(base.clone())).await;
        let first_body = first.ok(case).clone();
        let first_id = text(&first_body, "/id", case);
        let first_name = text(&first_body, "/name", case);
        let first_arn = text(&first_body, "/arn", case);

        let before = described(&emr, &first_id, case).await;
        assert_describes_the_minting_request(&before, &base, case);

        let mut conflicting = base.clone();
        (row.differ)(&mut conflicting, &other, case);
        assert_ne!(
            conflicting, base,
            "case {case:?}: the row left the request unchanged, so it tests nothing"
        );
        let refusal = emr
            .send("POST", "/virtualclusters", Some(conflicting))
            .await;
        refusal.refused(case, 400, "ValidationException");
        // The refusal is the TOKEN rule's, not some other `ValidationException`: it is the
        // one fixed sentence an idempotency conflict carries, whichever operation refuses
        // ([`crate::refusal_wording_by_error_class`] holds every operation to it).
        let message = text(&refusal.body, "/message", case);
        assert_eq!(
            message, SPENT_TOKEN_CONFLICT,
            "case {case:?}: expected message {SPENT_TOKEN_CONFLICT:?}, got {message:?}"
        );

        // Nothing the refused call asked for reached the store: the cluster reads back
        // exactly as it did before that call, and no second cluster was minted.
        let after = described(&emr, &first_id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused conflict must leave the cluster untouched, expected {before}, got {after}"
        );
        assert_describes_the_minting_request(&after, &base, case);
        let listed = listed_ids(&emr, case).await;
        assert_eq!(
            listed,
            vec![first_id.clone()],
            "case {case:?}: a refused conflict must persist nothing, expected only {first_id:?}, got {listed:?}"
        );

        // The other half of the rule: the SAME request again is a replay, so a retry is
        // answered with the cluster the first call minted rather than refused.
        let replay = emr.send("POST", "/virtualclusters", Some(base.clone())).await;
        let replay_body = replay.ok(case).clone();
        assert_eq!(
            (
                text(&replay_body, "/id", case),
                text(&replay_body, "/name", case),
                text(&replay_body, "/arn", case)
            ),
            (first_id.clone(), first_name.clone(), first_arn.clone()),
            "case {case:?}: the spent token must still name {first_id:?}"
        );
        let replayed_listing = listed_ids(&emr, case).await;
        assert_eq!(
            replayed_listing,
            vec![first_id.clone()],
            "case {case:?}: a replay must persist nothing new, got {replayed_listing:?}"
        );
    }
}
