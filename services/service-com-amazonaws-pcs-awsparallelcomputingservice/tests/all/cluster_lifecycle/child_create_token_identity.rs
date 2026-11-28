// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH resource a CREATE's `@idempotencyToken` is answerable for,
//! driven over the awsJson1_0 wire through the same router the emulator serves from, on
//! the harness [`crate::cluster_lifecycle`] builds.
//!
//! RULE under test: a `clientToken` retry "return[s] the result from the original
//! successful request" and has "no additional effect"
//! (`smithy_query(query="doc",shape="CreateComputeNodeGroupRequest",member="clientToken")`,
//! the same sentence on `CreateQueueRequest$clientToken`), so a request is that retry only
//! when it asks for the resource the first call MINTED — the same `@required` name inside
//! the same parent cluster. A request that carries the token while naming any other
//! resource is a different request: it is refused with the modelled `ConflictException`
//! naming the resource the token was used for, and nothing is stored for the name it asked
//! for.
//!
//! The suites are tables over the two creates that mint a resource INSIDE a cluster
//! (`CreateComputeNodeGroup`, `CreateQueue`), so the rule is checked wherever it holds and
//! not only on the operation it was reported on. Their rows differ along what the rule
//! generalises over: WHICH half of the resource's identity the retry changes — the name,
//! the parent cluster, or both.
//!
//! Nothing here waits on the wall clock: a transition this service owes is materialised
//! before the next request is answered.

use serde_json::{json, Value};
use service_com_amazonaws_pcs_awsparallelcomputingservice::service::wire::CLIENT_ERROR_STATUS;
use smithy_server_runtime::MultiServiceRouter;

use super::{call, create_cluster, create_compute_node_group, create_queue, emulator};

/// A token of a length `SBClientToken` admits
/// (`smithy_query(query="shape",shape="SBClientToken")`: `@length(min: 8, max: 100)`).
const TOKEN: &str = "scope-token-0001";

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// One create under test: the operation that mints a resource inside a cluster, and how
/// that resource is read back and listed.
#[derive(Debug, Clone, Copy)]
struct Create {
    /// The create operation's name.
    operation: &'static str,
    /// The output member the created resource travels in.
    member: &'static str,
    /// The noun the service's `clientToken` conflict names this kind of resource by.
    noun: &'static str,
    /// The `resourceType` that conflict reports.
    resource_type: &'static str,
    /// The read that answers with one such resource.
    reader: &'static str,
    /// The identifier member `reader` takes.
    reader_member: &'static str,
    /// The list that reports every such resource in a cluster.
    lister: &'static str,
    /// The output member `lister`'s page travels in.
    listed_member: &'static str,
    /// The name the FIRST call of each case creates its resource under.
    first_name: &'static str,
    /// Another name the resource's `@pattern` and `@length` admit, for the rows whose
    /// retry asks for a different resource.
    other_name: &'static str,
}

/// `CreateComputeNodeGroup`, scoped to its cluster.
const COMPUTE_NODE_GROUP: Create = Create {
    operation: "CreateComputeNodeGroup",
    member: "computeNodeGroup",
    noun: "compute node group",
    resource_type: "computenodegroup",
    reader: "GetComputeNodeGroup",
    reader_member: "computeNodeGroupIdentifier",
    lister: "ListComputeNodeGroups",
    listed_member: "computeNodeGroups",
    first_name: "scoped-nodes",
    other_name: "scoped-nodes-two",
};

/// `CreateQueue`, scoped to its cluster.
const QUEUE: Create = Create {
    operation: "CreateQueue",
    member: "queue",
    noun: "queue",
    resource_type: "queue",
    reader: "GetQueue",
    reader_member: "queueIdentifier",
    lister: "ListQueues",
    listed_member: "queues",
    first_name: "scoped-work",
    other_name: "scoped-work-two",
};

/// Every create under test.
const CREATES: [Create; 2] = [COMPUTE_NODE_GROUP, QUEUE];

/// One row: what the difference is called, whether the retry names the OTHER cluster, and
/// whether it asks for the other name.
type Difference = (&'static str, bool, bool);

/// Two clusters on one emulator, each observed `ACTIVE` by the request that follows its
/// create, so there is nothing to wait for.
async fn harness(case: &str) -> (MultiServiceRouter, String, String) {
    let router = emulator();
    let mut clusters = Vec::new();
    for name in ["scope-host-one", "scope-host-two"] {
        let (status, body) = call(&router, "CreateCluster", &create_cluster(name, "SMALL")).await;
        assert_eq!(
            status, 200,
            "case {case}: setup CreateCluster {name}: got {status} {body}"
        );
        clusters.push(
            body["cluster"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    let second = clusters.pop().expect("the second cluster was created");
    let first = clusters.pop().expect("the first cluster was created");
    (router, first, second)
}

/// The create request `create` takes for a resource called `name` in `cluster`, carrying
/// `token`. A queue associates no compute node group, which keeps every row of the table
/// the same request apart from the resource it names.
fn create_request(create: Create, cluster: &str, name: &str, token: &str) -> Value {
    let mut body = match create.operation {
        "CreateComputeNodeGroup" => create_compute_node_group(cluster, name),
        _ => create_queue(cluster, name, &[]),
    };
    body["clientToken"] = json!(token);
    body
}

/// The body `create.reader` takes for the resource `identifier` names in `cluster`.
fn read_request(create: Create, cluster: &str, identifier: &str) -> Value {
    json!({create.reader_member: identifier, "clusterIdentifier": cluster})
}

/// Every resource of `create`'s kind `cluster` holds.
async fn listed(
    router: &MultiServiceRouter,
    create: Create,
    cluster: &str,
    case: &str,
) -> Vec<Value> {
    let (status, page) = call(router, create.lister, &json!({"clusterIdentifier": cluster})).await;
    assert_eq!(
        status, 200,
        "case {case}: {} on {cluster}: got {status} {page}",
        create.lister
    );
    page[create.listed_member]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn a_create_token_naming_another_child_conflicts_and_creates_nothing() {
    // Rows: which half of the resource's identity the retry changes. The name and the
    // parent cluster together say WHICH resource a child create mints, so changing either
    // makes the request one the token's first call never made.
    let differences: [Difference; 3] = [
        ("another name in the same cluster", false, true),
        ("the same name in another cluster", true, false),
        ("another name in another cluster", true, true),
    ];

    for create in CREATES {
        for (difference, other_cluster, other_name) in differences {
            let case = format!("{} retried with {difference}", create.operation);
            let (router, first_cluster, second_cluster) = harness(&case).await;

            let (status, created) = call(
                &router,
                create.operation,
                &create_request(create, &first_cluster, create.first_name, TOKEN),
            )
            .await;
            assert_eq!(status, 200, "case {case}: first call: got {status} {created}");
            let first_id = created[create.member]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            let retry_cluster = if other_cluster {
                second_cluster.clone()
            } else {
                first_cluster.clone()
            };
            let retry_name = if other_name {
                create.other_name
            } else {
                create.first_name
            };
            let (status, refused) = call(
                &router,
                create.operation,
                &create_request(create, &retry_cluster, retry_name, TOKEN),
            )
            .await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (CLIENT_ERROR_STATUS, "ConflictException"),
                "case {case}: expected the reused token to conflict, got {status} {refused}"
            );
            assert_eq!(
                refused["message"],
                json!(format!(
                    "clientToken was already used to create {} {first_id}",
                    create.noun
                )),
                "case {case}: expected the refusal to name the resource the token created, \
                 got {refused}"
            );
            assert_eq!(
                (
                    refused["resourceId"].as_str(),
                    refused["resourceType"].as_str()
                ),
                (Some(first_id.as_str()), Some(create.resource_type)),
                "case {case}: expected the refusal to carry the first call's resource, got \
                 {refused}"
            );

            // Nothing was stored for the resource the retry asked for.
            let (status, missing) = call(
                &router,
                create.reader,
                &read_request(create, &retry_cluster, retry_name),
            )
            .await;
            assert_eq!(
                (status, error_code(&missing).as_str()),
                (CLIENT_ERROR_STATUS, "ResourceNotFoundException"),
                "case {case}: expected {retry_name} in {retry_cluster} not to have been \
                 created, got {status} {missing}"
            );

            // The first call's resource is the only one of its kind in its cluster, and
            // the cluster the retry named holds nothing of its own.
            let first_page = listed(&router, create, &first_cluster, &case).await;
            assert_eq!(
                first_page.len(),
                1,
                "case {case}: expected the first call's resource to be the only one in \
                 {first_cluster}, got {first_page:?}"
            );
            assert_eq!(
                first_page
                    .first()
                    .and_then(|entry| entry["id"].as_str()),
                Some(first_id.as_str()),
                "case {case}: expected the first call's resource to still be listed, got \
                 {first_page:?}"
            );
            let second_page = listed(&router, create, &second_cluster, &case).await;
            assert!(
                second_page.is_empty(),
                "case {case}: expected the other cluster to hold nothing, got \
                 {second_page:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_create_token_naming_the_same_child_replays_the_first_call() {
    // The other side of the same rule: the retry that asks for the resource the first call
    // minted — the same name in the same cluster — is answered with that resource and
    // creates nothing, in either cluster.
    for create in CREATES {
        let case = format!("{} retried for the same resource", create.operation);
        let (router, first_cluster, second_cluster) = harness(&case).await;

        let (status, created) = call(
            &router,
            create.operation,
            &create_request(create, &first_cluster, create.first_name, TOKEN),
        )
        .await;
        assert_eq!(status, 200, "case {case}: first call: got {status} {created}");
        let created = created[create.member].clone();

        let (status, replayed) = call(
            &router,
            create.operation,
            &create_request(create, &first_cluster, create.first_name, TOKEN),
        )
        .await;
        assert_eq!(status, 200, "case {case}: retry: got {status} {replayed}");
        let replayed = replayed[create.member].clone();
        assert_eq!(
            (
                replayed["id"].as_str(),
                replayed["name"].as_str(),
                replayed["arn"].as_str()
            ),
            (
                created["id"].as_str(),
                created["name"].as_str(),
                created["arn"].as_str()
            ),
            "case {case}: expected the first call's resource, got {replayed}"
        );

        assert_eq!(
            listed(&router, create, &first_cluster, &case).await.len(),
            1,
            "case {case}: expected the replay to have created nothing in {first_cluster}"
        );
        assert!(
            listed(&router, create, &second_cluster, &case)
                .await
                .is_empty(),
            "case {case}: expected the other cluster to hold nothing"
        );
    }
}
