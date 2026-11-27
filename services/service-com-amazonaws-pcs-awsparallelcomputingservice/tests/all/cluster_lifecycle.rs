// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PCS cluster lifecycle, driven over the awsJson1_0 wire
//! (`POST /` with `X-Amz-Target: AWSParallelComputingService.<Operation>`) through the
//! same router the emulator serves from — so these exercise routing, deserialization,
//! constraint validation, the handler and serialization, not a handler in isolation.
//!
//! Each test builds its own engine, so the state one test writes is invisible to the
//! others.
//!
//! WHEN a lifecycle transition becomes observable is its own suite, in the submodule
//! below (`tests/all/cluster_lifecycle/settles_on_the_next_request.rs`): a transition this
//! service owes is materialised before the NEXT request is answered, so the tests here
//! never wait on the wall clock for one.

// `tests/all/cluster_lifecycle.rs` is itself loaded by `#[path]` from `tests/all.rs`, so
// a submodule of it is looked up beside it rather than in a directory named after it: the
// `#[path]` here is what points at `tests/all/cluster_lifecycle/`. The layout check
// declares suites DIRECTLY under `tests/all/`; a file deeper than that is declared by its
// own parent module, which is this one.
#[path = "cluster_lifecycle/settles_on_the_next_request.rs"]
mod settles_on_the_next_request;

// What a chain of lifecycle waits COSTS a polling client, and the teardown legs it only
// reaches once each of them has settled, are their own suite, declared the same way.
#[path = "cluster_lifecycle/teardown_tail_is_reachable.rs"]
mod teardown_tail_is_reachable;

// The associations a stored queue carries are their own suite, declared the same way.
#[path = "cluster_lifecycle/queue_associations.rs"]
mod queue_associations;

// What a reused `clientToken` on a CREATE answers is its own suite, declared the same way.
#[path = "cluster_lifecycle/idempotent_create_retry.rs"]
mod idempotent_create_retry;

// What a CREATE carrying a token whose resource has since been deleted answers is its own
// suite, declared the same way.
#[path = "cluster_lifecycle/recreate_with_a_used_client_token.rs"]
mod recreate_with_a_used_client_token;

// WHICH members a create and a read report as SET because the model documents the value the
// service applies for them, declared the same way.
#[path = "cluster_lifecycle/documented_member_defaults.rs"]
mod documented_member_defaults;

use serde_json::{json, Value};
use service_com_amazonaws_pcs_awsparallelcomputingservice::service::wire::CLIENT_ERROR_STATUS;
use service_com_amazonaws_pcs_awsparallelcomputingservice::state::{
    MAX_CLUSTERS, MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER, MAX_QUEUES_PER_CLUSTER,
    MAX_STORED_LIST_ENTRIES,
};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to `pcs` in `region`. The router resolves a
/// request's service from the credential scope, and the handlers read the region out of
/// it to build an ARN. The signature is never checked: request authentication is off.
pub(crate) fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/pcs/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// The region [`call`] signs for, which every ARN in these tests carries.
pub(crate) const REGION: &str = "us-east-1";

/// An emulator with only PCS mounted, on its own engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// [`emulator`], handing back the engine it was built on as well, for a test that has to
/// put a record into a state no sequence of requests can leave it in.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>)
{
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_pcs_awsparallelcomputingservice::EmulatedService::new(engine.clone()),
    );
    (router, engine)
}

/// Call one operation, signed for [`REGION`], and return its HTTP status and JSON body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_signed_for(router, REGION, operation, body).await
}

/// [`call`], signing for `region`: the ONE request builder these suites drive the wire with,
/// so a test that varies the credential scope's Region exercises exactly the path every
/// other test exercises.
pub(crate) async fn call_signed_for(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("AWSParallelComputingService.{operation}"),
        )
        .header("authorization", signed_for(region))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body().clone();
    let parsed = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered {status} with a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// `resource` without `members`, so what a create stored can be compared WHOLE against
/// what the next read answers, minus only the members a settled lifecycle transition
/// moves.
fn without_members(resource: &Value, members: &[&str]) -> Value {
    let mut stripped = resource.clone();
    if let Some(object) = stripped.as_object_mut() {
        for member in members {
            object.remove(*member);
        }
    }
    stripped
}

/// A `CreateCluster` request carrying every `@required` member.
pub(crate) fn create_cluster(name: &str, size: &str) -> Value {
    json!({
        "clusterName": name,
        "scheduler": {"type": "SLURM", "version": "24.11"},
        "size": size,
        "networking": {
            "subnetIds": ["subnet-0123456789abcdef0"],
            "securityGroupIds": ["sg-0123456789abcdef0"],
            "networkType": "IPV4",
        },
    })
}

/// A `CreateComputeNodeGroup` request carrying every `@required` member. The instance
/// profile ARN satisfies `InstanceProfileArn`'s `@pattern`, which the boundary enforces.
pub(crate) fn create_compute_node_group(cluster: &str, name: &str) -> Value {
    json!({
        "clusterIdentifier": cluster,
        "computeNodeGroupName": name,
        "subnetIds": ["subnet-0123456789abcdef0"],
        "customLaunchTemplate": {"id": "lt-0123456789abcdef0", "version": "1"},
        "iamInstanceProfileArn": format!(
            "arn:aws:iam::{}:instance-profile/AWSPCS-demo",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "scalingConfiguration": {"minInstanceCount": 0, "maxInstanceCount": 4},
        "instanceConfigs": [{"instanceType": "c6i.large"}],
    })
}

/// A `CreateQueue` request, assigning the queue to `groups`.
pub(crate) fn create_queue(cluster: &str, name: &str, groups: &[&str]) -> Value {
    json!({
        "clusterIdentifier": cluster,
        "queueName": name,
        "computeNodeGroupConfigurations": groups
            .iter()
            .map(|id| json!({"computeNodeGroupId": id}))
            .collect::<Vec<Value>>(),
    })
}

/// The stored cluster `identifier` names, as `GetCluster` reports it — the one reader every
/// suite in this target reads a cluster back through, so no two of them read it differently.
pub(crate) async fn stored_cluster(router: &MultiServiceRouter, identifier: &str) -> Value {
    let (status, read) = call(
        router,
        "GetCluster",
        &json!({"clusterIdentifier": identifier}),
    )
    .await;
    assert_eq!(
        status, 200,
        "read back {identifier}: got {status} with {read}"
    );
    read["cluster"].clone()
}

/// Create a cluster and answer its id. The request that follows the create observes it
/// `ACTIVE`, so there is nothing to wait for.
async fn active_cluster(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "CreateCluster", &create_cluster(name, "SMALL")).await;
    assert_eq!(status, 200, "case create {name}: got {body}");
    body["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_cluster_round_trips_through_create_get_list_and_delete() {
    let router = emulator();

    let (status, created) = call(
        &router,
        "CreateCluster",
        &create_cluster("demo-cluster", "SMALL"),
    )
    .await;
    assert_eq!(status, 200, "case CreateCluster: got {created}");
    let cluster = created["cluster"].clone();
    let id = cluster["id"].as_str().unwrap_or_default().to_string();

    assert!(
        id.starts_with("pcs_") && id.len() > "pcs_".len(),
        "case CreateCluster: expected a generated pcs_ id, got {cluster}"
    );
    assert_eq!(
        cluster["arn"],
        json!(format!(
            "arn:aws:pcs:{REGION}:{}:cluster/{id}",
            engine::EMULATOR_ACCOUNT_ID
        )),
        "case CreateCluster: expected the cluster ARN to carry the signed region, the \
         emulator account and the generated id, got {cluster}"
    );
    assert_eq!(
        cluster["status"],
        json!("CREATING"),
        "case CreateCluster: expected a new cluster to be reported CREATING, got {cluster}"
    );
    assert!(
        !cluster["createdAt"].is_null() && !cluster["modifiedAt"].is_null(),
        "case CreateCluster: expected both required timestamps, got {cluster}"
    );

    // The identifier resolves by either of the two forms `ClusterIdentifier` allows, and
    // both answer the record the create stored, with the transition the create owed
    // already materialised: the VERY NEXT request after the create reports ACTIVE.
    for identifier in [id.as_str(), "demo-cluster"] {
        let (status, got) = call(
            &router,
            "GetCluster",
            &json!({"clusterIdentifier": identifier}),
        )
        .await;
        assert_eq!(status, 200, "case GetCluster {identifier}: got {got}");
        assert_eq!(
            got["cluster"]["status"],
            json!("ACTIVE"),
            "case GetCluster {identifier}: expected the owed CREATING -> ACTIVE transition \
             on the request that follows the create, got {got}"
        );
        // Everything else the create stored reads back unchanged, compared WHOLE with only
        // the members a settled transition moves taken out: `status` is asserted above,
        // `modifiedAt` is set when the owed change is materialised, and `endpoints` are
        // published once the cluster is provisioned (their own suite is
        // `tests/all/cluster_update_and_endpoints.rs`). `createdAt` is in the comparison, so
        // it is shown stable across reads.
        let moved_by_the_transition = ["status", "modifiedAt", "endpoints"];
        assert_eq!(
            without_members(&got["cluster"], &moved_by_the_transition),
            without_members(&cluster, &moved_by_the_transition),
            "case GetCluster {identifier}: expected every other member exactly as \
             CreateCluster stored it, got {got}"
        );
    }

    // A second cluster of the same name conflicts; the name stays bound to the first id.
    let (status, conflict) = call(
        &router,
        "CreateCluster",
        &create_cluster("demo-cluster", "LARGE"),
    )
    .await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case duplicate CreateCluster: expected a conflict, got {conflict}"
    );
    assert_eq!(
        conflict["__type"],
        json!("ConflictException"),
        "case duplicate CreateCluster: got {conflict}"
    );

    let (status, missing) = call(
        &router,
        "GetCluster",
        &json!({"clusterIdentifier": "no-such-cluster"}),
    )
    .await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case GetCluster on an unknown identifier: got {missing}"
    );
    assert_eq!(
        missing["__type"],
        json!("ResourceNotFoundException"),
        "case GetCluster on an unknown identifier: got {missing}"
    );

    let (status, listed) = call(&router, "ListClusters", &json!({})).await;
    assert_eq!(status, 200, "case ListClusters: got {listed}");
    let summaries = listed["clusters"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "case ListClusters: expected the one persisted cluster, got {listed}"
    );
    assert_eq!(
        summaries[0]["id"],
        json!(&id),
        "case ListClusters: expected the summary to carry the stored id, got {listed}"
    );

    // Delete reports DELETING on the record it moves, and the removal it owes is
    // materialised before the next request is answered: the read that follows the delete
    // is already a miss, by either identifier form.
    let (status, deleted) = call(
        &router,
        "DeleteCluster",
        &json!({"clusterIdentifier": "demo-cluster"}),
    )
    .await;
    assert_eq!(status, 200, "case DeleteCluster: got {deleted}");

    for identifier in [id.as_str(), "demo-cluster"] {
        let (status, gone) = call(
            &router,
            "GetCluster",
            &json!({"clusterIdentifier": identifier}),
        )
        .await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case GetCluster {identifier} after the delete came due: got {gone}"
        );
    }
    let (status, empty) = call(&router, "ListClusters", &json!({})).await;
    assert_eq!(status, 200, "case ListClusters after delete: got {empty}");
    assert_eq!(
        empty["clusters"],
        json!([]),
        "case ListClusters after delete: expected no clusters, got {empty}"
    );
}

#[tokio::test]
async fn a_compute_node_group_and_a_queue_round_trip_through_their_readers() {
    let router = emulator();
    let cluster_id = active_cluster(&router, "demo-cluster").await;

    let (status, created) = call(
        &router,
        "CreateComputeNodeGroup",
        &create_compute_node_group("demo-cluster", "demo-cng"),
    )
    .await;
    assert_eq!(status, 200, "case CreateComputeNodeGroup: got {created}");
    let group = created["computeNodeGroup"].clone();
    let group_id = group["id"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        group["clusterId"],
        json!(&cluster_id),
        "case CreateComputeNodeGroup: expected the parent cluster's id, got {group}"
    );
    assert_eq!(
        group["arn"],
        json!(format!(
            "arn:aws:pcs:{REGION}:{}:cluster/{cluster_id}/computenodegroup/{group_id}",
            engine::EMULATOR_ACCOUNT_ID
        )),
        "case CreateComputeNodeGroup: got {group}"
    );
    assert_eq!(
        group["scalingConfiguration"],
        json!({"minInstanceCount": 0, "maxInstanceCount": 4}),
        "case CreateComputeNodeGroup: expected the requested scaling bounds, got {group}"
    );

    // Read it back through its own reader: the create persisted it, and the request that
    // follows the create reports the group ACTIVE.
    for identifier in [group_id.as_str(), "demo-cng"] {
        let (status, got) = call(
            &router,
            "GetComputeNodeGroup",
            &json!({
                "clusterIdentifier": &cluster_id,
                "computeNodeGroupIdentifier": identifier,
            }),
        )
        .await;
        assert_eq!(status, 200, "case GetComputeNodeGroup {identifier}: got {got}");
        assert_eq!(
            got["computeNodeGroup"]["status"],
            json!("ACTIVE"),
            "case GetComputeNodeGroup {identifier}: expected the owed CREATING -> ACTIVE \
             transition on the request that follows the create, got {got}"
        );
        // As for the cluster: compared whole, without the two members the settled
        // transition moves.
        let moved_by_the_transition = ["status", "modifiedAt"];
        assert_eq!(
            without_members(&got["computeNodeGroup"], &moved_by_the_transition),
            without_members(&group, &moved_by_the_transition),
            "case GetComputeNodeGroup {identifier}: expected every other member exactly as \
             the create stored it, got {got}"
        );
    }

    let (status, created) = call(
        &router,
        "CreateQueue",
        &create_queue("demo-cluster", "demo-queue", &[group_id.as_str()]),
    )
    .await;
    assert_eq!(status, 200, "case CreateQueue: got {created}");
    let queue = created["queue"].clone();
    let queue_id = queue["id"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        queue["clusterId"],
        json!(&cluster_id),
        "case CreateQueue: expected the parent cluster's id, got {queue}"
    );
    assert_eq!(
        queue["arn"],
        json!(format!(
            "arn:aws:pcs:{REGION}:{}:cluster/{cluster_id}/queue/{queue_id}",
            engine::EMULATOR_ACCOUNT_ID
        )),
        "case CreateQueue: got {queue}"
    );
    assert_eq!(
        queue["computeNodeGroupConfigurations"],
        json!([{"computeNodeGroupId": &group_id}]),
        "case CreateQueue: expected the requested compute node groups, got {queue}"
    );

    for identifier in [queue_id.as_str(), "demo-queue"] {
        let (status, got) = call(
            &router,
            "GetQueue",
            &json!({"clusterIdentifier": "demo-cluster", "queueIdentifier": identifier}),
        )
        .await;
        assert_eq!(status, 200, "case GetQueue {identifier}: got {got}");
        assert_eq!(
            got["queue"]["status"],
            json!("ACTIVE"),
            "case GetQueue {identifier}: expected the owed CREATING -> ACTIVE transition on \
             the request that follows the create, got {got}"
        );
        let moved_by_the_transition = ["status", "modifiedAt"];
        assert_eq!(
            without_members(&got["queue"], &moved_by_the_transition),
            without_members(&queue, &moved_by_the_transition),
            "case GetQueue {identifier}: expected every other member exactly as the create \
             stored it, got {got}"
        );
    }

    let (status, listed) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": "demo-cluster"}),
    )
    .await;
    assert_eq!(status, 200, "case ListQueues: got {listed}");
    assert_eq!(
        listed["queues"].as_array().map(Vec::len),
        Some(1),
        "case ListQueues: expected the one persisted queue, got {listed}"
    );
    assert_eq!(
        listed["queues"][0]["id"],
        json!(&queue_id),
        "case ListQueues: got {listed}"
    );

    // A cluster that still holds either child cannot be deleted.
    let (status, refused) = call(
        &router,
        "DeleteCluster",
        &json!({"clusterIdentifier": &cluster_id}),
    )
    .await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case DeleteCluster with children: expected a conflict, got {refused}"
    );
}

#[tokio::test]
async fn a_child_create_refuses_a_cluster_it_cannot_find() {
    let router = emulator();
    active_cluster(&router, "demo-cluster").await;

    // Rows: the operation and the request. Every row names a cluster that does not exist,
    // in each of the two forms `ClusterIdentifier` allows, and the refusal each form earns:
    // the generated id is the not-found, the name the permissions failure.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "CreateComputeNodeGroup",
            create_compute_node_group("no-such-cluster", "demo-cng"),
            "AccessDeniedException",
        ),
        (
            "CreateComputeNodeGroup",
            create_compute_node_group("pcs_0000000000000000", "demo-cng"),
            "ResourceNotFoundException",
        ),
        (
            // Each queue row associates a compute node group, as a stored queue must; the
            // row is about the cluster its identifier fails to resolve to.
            "CreateQueue",
            create_queue("no-such-cluster", "demo-queue", &["demo-cng"]),
            "AccessDeniedException",
        ),
        (
            "CreateQueue",
            create_queue("pcs_0000000000000000", "demo-queue", &["demo-cng"]),
            "ResourceNotFoundException",
        ),
        ("GetQueue", json!({
            "clusterIdentifier": "no-such-cluster",
            "queueIdentifier": "demo-queue",
        }), "AccessDeniedException"),
        (
            "ListQueues",
            json!({"clusterIdentifier": "no-such-cluster"}),
            "AccessDeniedException",
        ),
        ("GetComputeNodeGroup", json!({
            "clusterIdentifier": "no-such-cluster",
            "computeNodeGroupIdentifier": "demo-cng",
        }), "AccessDeniedException"),
        ("GetComputeNodeGroup", json!({
            "clusterIdentifier": "pcs_0000000000000000",
            "computeNodeGroupIdentifier": "demo-cng",
        }), "ResourceNotFoundException"),
    ];
    for (operation, request, expected) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {operation} on an unknown cluster: expected the client-error status, got \
             {status} {body}"
        );
        // Which refusal a child operation earns is decided by the FORM of the identifier it
        // named: a generated cluster id names a cluster that is not there, a cluster name
        // cannot be authorized at all.
        assert_eq!(
            body["__type"],
            json!(expected),
            "case {operation} on an unknown cluster: expected {expected}, got {body}"
        );
    }

    // A queue cannot be assigned a compute node group its own cluster does not hold.
    let (status, body) = call(
        &router,
        "CreateQueue",
        &create_queue("demo-cluster", "demo-queue", &["pcs_0000000000000000"]),
    )
    .await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case CreateQueue naming an absent compute node group: got {status} {body}"
    );

    // A scaling range the cluster could never satisfy is refused. Rows: an inverted
    // range, and the degenerate-but-legal equal one.
    for (min, max, expected) in [(4, 0, 400_u16), (0, 0, 200), (2, 2, 200)] {
        let mut request = create_compute_node_group("demo-cluster", &format!("scale-{min}-{max}"));
        request["scalingConfiguration"] =
            json!({"minInstanceCount": min, "maxInstanceCount": max});
        let (status, body) = call(&router, "CreateComputeNodeGroup", &request).await;
        assert_eq!(
            status, expected,
            "case scalingConfiguration {min}..{max}: expected {expected}, got {status} {body}"
        );
    }

    // A queue or compute node group the cluster does not hold is not found either.
    for (operation, request) in [
        (
            "GetQueue",
            json!({"clusterIdentifier": "demo-cluster", "queueIdentifier": "no-such-queue"}),
        ),
        (
            "GetComputeNodeGroup",
            json!({
                "clusterIdentifier": "demo-cluster",
                "computeNodeGroupIdentifier": "no-such-cng",
            }),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {operation} on an unknown child: expected the client-error status, got \
             {status} {body}"
        );
        // The cluster DID resolve, so the refusal is about the child the request named.
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {operation} on an unknown child: got {body}"
        );
    }
}

#[tokio::test]
async fn a_resource_name_is_accepted_exactly_when_it_satisfies_its_pattern_and_length() {
    let router = emulator();

    // `ClusterName` (and `ComputeNodeGroupName`, and `QueueName`) is
    // `@length(min: 3, max: 40)` with `@pattern("^(?!pcs_)^[A-Za-z][A-Za-z0-9-]+$")` —
    // from `smithy_query(query="operation", operation="CreateCluster")`. Rows span the
    // class the rule generalises over: each published length bound and one past it, and
    // each character the class admits or refuses.
    let at_max = "a".repeat(40);
    let over_max = "a".repeat(41);
    let cases: Vec<(&str, u16)> = vec![
        ("demo-cluster", 200),      // the common form
        ("Demo-Cluster-9", 200),    // upper case and digits are in the class
        ("abc", 200),               // the @length minimum
        (at_max.as_str(), 200),     // the @length maximum
        ("ab", 400),                // one under the @length minimum
        (over_max.as_str(), 400),   // one over the @length maximum
        ("pcs_demo", 400),          // the prefix reserved for generated ids
        ("1demo", 400),             // must start with a letter
        ("-demo", 400),             // ... a letter, not a hyphen
        ("demo_cluster", 400),      // `_` is not in the class
        ("demo cluster", 400),      // nor is a space
        ("démo", 400),              // nor is a non-ASCII letter
    ];
    for (name, expected) in cases {
        let (status, body) = call(&router, "CreateCluster", &create_cluster(name, "SMALL")).await;
        assert_eq!(
            status, expected,
            "case clusterName={name:?}: expected {expected}, got {status} {body}"
        );
        if expected == 400 {
            assert_eq!(
                body["__type"],
                json!("ValidationException"),
                "case clusterName={name:?}: got {body}"
            );
        }
    }

    // The same rule holds for the other two name members, on a value neither the issue
    // nor the rows above cite.
    let cluster_id = active_cluster(&router, "names-cluster").await;
    for (operation, request, member) in [
        (
            "CreateComputeNodeGroup",
            create_compute_node_group(&cluster_id, "pcs_group"),
            "computeNodeGroupName",
        ),
        (
            "CreateQueue",
            create_queue(&cluster_id, "queue.one", &[]),
            "queueName",
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {member} outside its pattern: expected 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case {member} outside its pattern: got {body}"
        );
    }
}

#[tokio::test]
async fn a_cluster_size_is_accepted_exactly_when_the_size_enum_declares_it() {
    let router = emulator();

    // The `Size` enum's members, from `SIZE_VALUES` in the service's generated types
    // (`smithy_query(query="shape", shape="Size")`), plus values outside it.
    let cases: Vec<(&str, u16)> = vec![
        ("SMALL", 200),
        ("MEDIUM", 200),
        ("LARGE", 200),
        ("small", 400),      // the members are upper case
        ("EXTRALARGE", 400), // not a member
        ("", 400),           // not a member
    ];
    for (index, (size, expected)) in cases.iter().enumerate() {
        let name = format!("size-cluster-{index}");
        let (status, body) = call(&router, "CreateCluster", &create_cluster(&name, size)).await;
        assert_eq!(
            status, *expected,
            "case size={size:?}: expected {expected}, got {status} {body}"
        );
        if *expected == 200 {
            assert_eq!(
                body["cluster"]["size"],
                json!(size),
                "case size={size:?}: expected the stored size to be echoed, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn list_clusters_pages_through_every_cluster_at_every_page_size() {
    let router = emulator();

    let names = ["alpha-cluster", "beta-cluster", "gamma-cluster"];
    let mut created = Vec::new();
    for name in names {
        let (status, body) = call(&router, "CreateCluster", &create_cluster(name, "SMALL")).await;
        assert_eq!(status, 200, "case create {name}: got {body}");
        created.push(
            body["cluster"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    // Rows: the page size, spanning one item per page up to more than the whole set.
    for page_size in [1_usize, 2, 3, 4] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListClusters", &request).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size} page {pages}: got {body}"
            );
            let page = body["clusters"].as_array().cloned().unwrap_or_default();
            pages += 1;
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case maxResults={page_size} page {pages}: expected 1..={page_size} rows, \
                 got {} in {body}",
                page.len()
            );
            seen.extend(
                page.iter()
                    .map(|row| row["id"].as_str().unwrap_or_default().to_string()),
            );
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= names.len(),
                "case maxResults={page_size}: pagination did not terminate"
            );
        }
        assert_eq!(
            seen, created,
            "case maxResults={page_size}: expected every cluster exactly once in creation \
             order, got {seen:?}"
        );
        let expected_pages = created.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case maxResults={page_size}: expected {expected_pages} pages, got {pages}"
        );
    }

    // A token this service did not issue names no page, and is refused rather than
    // served as a first page.
    for token in ["not-hex", "6162", ""] {
        let (status, body) = call(
            &router,
            "ListClusters",
            &json!({"nextToken": token, "maxResults": 1}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case nextToken={token:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case nextToken={token:?}: got {body}"
        );
    }

    // A token is scoped to the listing that issued it: the creation sequence is
    // service-wide, so a `ListClusters` token must not resume a `ListQueues` page.
    let (status, page) = call(&router, "ListClusters", &json!({"maxResults": 1})).await;
    assert_eq!(status, 200, "case ListClusters page one: got {page}");
    let cluster_token = page["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !cluster_token.is_empty(),
        "case ListClusters page one: expected a nextToken, got {page}"
    );
    let (status, refused) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": &created[0], "nextToken": &cluster_token}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a ListClusters token replayed into ListQueues: expected 400, got \
         {status} {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ValidationException"),
        "case a ListClusters token replayed into ListQueues: got {refused}"
    );
}

#[tokio::test]
async fn an_idempotency_token_replays_the_first_create_instead_of_conflicting() {
    let router = emulator();

    // Rows: the operation, its first request, and a retry of it. A retry carrying the
    // same `clientToken` returns the first call's resource and has no further effect.
    let cluster_token = "cluster-token-0001";
    let mut first_cluster = create_cluster("demo-cluster", "SMALL");
    first_cluster["clientToken"] = json!(cluster_token);

    let (status, created) = call(&router, "CreateCluster", &first_cluster).await;
    assert_eq!(status, 200, "case first CreateCluster: got {created}");
    let (status, replayed) = call(&router, "CreateCluster", &first_cluster).await;
    assert_eq!(status, 200, "case CreateCluster retry: got {replayed}");
    assert_eq!(
        replayed["cluster"]["id"], created["cluster"]["id"],
        "case CreateCluster retry: expected the first call's cluster, got {replayed}"
    );

    // A DIFFERENT token on the same name is a genuine second create, so it conflicts.
    let mut other = create_cluster("demo-cluster", "SMALL");
    other["clientToken"] = json!("cluster-token-0002");
    let (status, conflict) = call(&router, "CreateCluster", &other).await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case CreateCluster with a new token on a taken name: got {status} {conflict}"
    );

    let cluster_id = created["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let mut first_group = create_compute_node_group(&cluster_id, "demo-cng");
    first_group["clientToken"] = json!("group-token-0001");
    let (status, group) = call(&router, "CreateComputeNodeGroup", &first_group).await;
    assert_eq!(status, 200, "case first CreateComputeNodeGroup: got {group}");
    let (status, group_retry) = call(&router, "CreateComputeNodeGroup", &first_group).await;
    assert_eq!(
        status, 200,
        "case CreateComputeNodeGroup retry: got {group_retry}"
    );
    assert_eq!(
        group_retry["computeNodeGroup"]["id"], group["computeNodeGroup"]["id"],
        "case CreateComputeNodeGroup retry: expected the first call's group, got {group_retry}"
    );

    let mut first_queue = create_queue(&cluster_id, "demo-queue", &["demo-cng"]);
    first_queue["clientToken"] = json!("queue-token-0001");
    let (status, queue) = call(&router, "CreateQueue", &first_queue).await;
    assert_eq!(status, 200, "case first CreateQueue: got {queue}");
    let (status, queue_retry) = call(&router, "CreateQueue", &first_queue).await;
    assert_eq!(status, 200, "case CreateQueue retry: got {queue_retry}");
    assert_eq!(
        queue_retry["queue"]["id"], queue["queue"]["id"],
        "case CreateQueue retry: expected the first call's queue, got {queue_retry}"
    );

    // One cluster, one group, one queue — the retries created nothing.
    let (status, listed) = call(&router, "ListClusters", &json!({})).await;
    assert_eq!(status, 200, "case ListClusters: got {listed}");
    assert_eq!(
        listed["clusters"].as_array().map(Vec::len),
        Some(1),
        "case ListClusters: expected exactly one cluster, got {listed}"
    );
    let (status, queues) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": &cluster_id}),
    )
    .await;
    assert_eq!(status, 200, "case ListQueues: got {queues}");
    assert_eq!(
        queues["queues"].as_array().map(Vec::len),
        Some(1),
        "case ListQueues: expected exactly one queue, got {queues}"
    );
}

#[tokio::test]
async fn a_missing_required_member_is_refused_before_any_record_is_written() {
    let router = emulator();

    // Rows: each `@required` member of `CreateCluster`, dropped in turn.
    for member in ["clusterName", "scheduler", "size", "networking"] {
        let mut request = create_cluster("demo-cluster", "SMALL");
        request
            .as_object_mut()
            .expect("the request is a JSON object")
            .remove(member);
        let (status, body) = call(&router, "CreateCluster", &request).await;
        assert_eq!(
            status, 400,
            "case {member} absent: expected 400, got {status} {body}"
        );
    }

    let (status, listed) = call(&router, "ListClusters", &json!({})).await;
    assert_eq!(status, 200, "case ListClusters: got {listed}");
    assert_eq!(
        listed["clusters"],
        json!([]),
        "case ListClusters after refused creates: expected no cluster to have been \
         written, got {listed}"
    );
}

#[tokio::test]
async fn an_idempotency_token_cannot_be_replayed_into_another_cluster() {
    let router = emulator();
    let first_cluster = active_cluster(&router, "first-cluster").await;
    let (status, body) = call(
        &router,
        "CreateCluster",
        &create_cluster("second-cluster", "SMALL"),
    )
    .await;
    assert_eq!(status, 200, "case create second-cluster: got {body}");
    let second_cluster = body["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A compute node group in each cluster, because every queue below associates one.
    for cluster in [&first_cluster, &second_cluster] {
        let (status, group) = call(
            &router,
            "CreateComputeNodeGroup",
            &create_compute_node_group(cluster, "demo-cng"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case create demo-cng in {cluster}: got {status} {group}"
        );
    }

    let token = "shared-token-0001";
    let mut request = create_queue(&first_cluster, "demo-queue", &["demo-cng"]);
    request["clientToken"] = json!(token);
    let (status, created) = call(&router, "CreateQueue", &request).await;
    assert_eq!(status, 200, "case first CreateQueue: got {created}");

    // The token names the first call's queue, in the cluster that call created it in.
    // Replaying it against another cluster must not create a second queue.
    let mut elsewhere = create_queue(&second_cluster, "demo-queue", &["demo-cng"]);
    elsewhere["clientToken"] = json!(token);
    let (status, refused) = call(&router, "CreateQueue", &elsewhere).await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case replay into another cluster: expected a conflict, got {status} {refused}"
    );
    let (status, queues) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": &second_cluster}),
    )
    .await;
    assert_eq!(status, 200, "case ListQueues on the other cluster: {queues}");
    assert_eq!(
        queues["queues"],
        json!([]),
        "case replay into another cluster: expected no queue to have been written there, \
         got {queues}"
    );

    // A queue page token is scoped to ITS cluster: the creation sequence is
    // service-wide, so one cluster's token must not resume another cluster's queues.
    let (status, body) = call(
        &router,
        "CreateQueue",
        &create_queue(&first_cluster, "second-queue", &["demo-cng"]),
    )
    .await;
    assert_eq!(status, 200, "case create second-queue: got {body}");
    let (status, page) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": &first_cluster, "maxResults": 1}),
    )
    .await;
    assert_eq!(status, 200, "case ListQueues page one: got {page}");
    let queue_token = page["nextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !queue_token.is_empty(),
        "case ListQueues page one: expected a nextToken, got {page}"
    );
    let (status, refused) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": &second_cluster, "nextToken": &queue_token}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one cluster's queue token replayed into another cluster: expected 400, \
         got {status} {refused}"
    );
}

#[tokio::test]
async fn a_create_past_a_store_ceiling_is_refused_with_the_modelled_quota_error() {
    let router = emulator();

    // The cluster store's ceiling: every create up to it succeeds, the next is refused.
    for index in 0..MAX_CLUSTERS {
        let name = format!("cap-cluster-{index}");
        let (status, body) = call(&router, "CreateCluster", &create_cluster(&name, "SMALL")).await;
        assert_eq!(
            status, 200,
            "case cluster {index} of {MAX_CLUSTERS}: expected 200, got {status} {body}"
        );
    }
    let (status, refused) = call(
        &router,
        "CreateCluster",
        &create_cluster("cap-cluster-over", "SMALL"),
    )
    .await;
    assert_eq!(
        (status, refused["__type"].as_str()),
        (CLIENT_ERROR_STATUS, Some("ServiceQuotaExceededException")),
        "case one cluster past the ceiling: expected the modelled quota error, got \
         {status} {refused}"
    );
}

#[tokio::test]
async fn a_child_create_past_its_cluster_ceiling_is_refused_with_the_modelled_quota_error() {
    let router = emulator();
    let cluster = active_cluster(&router, "cap-cluster").await;

    // Rows: each per-cluster ceiling, with the create that fills it and the create that
    // goes one past.
    for index in 0..MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER {
        let name = format!("cap-cng-{index}");
        let (status, body) = call(
            &router,
            "CreateComputeNodeGroup",
            &create_compute_node_group(&cluster, &name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case compute node group {index} of {MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER}: \
             expected 200, got {status} {body}"
        );
    }
    let (status, refused) = call(
        &router,
        "CreateComputeNodeGroup",
        &create_compute_node_group(&cluster, "cap-cng-over"),
    )
    .await;
    assert_eq!(
        (status, refused["__type"].as_str()),
        (CLIENT_ERROR_STATUS, Some("ServiceQuotaExceededException")),
        "case one compute node group past the ceiling: got {status} {refused}"
    );

    for index in 0..MAX_QUEUES_PER_CLUSTER {
        let name = format!("cap-queue-{index}");
        let (status, body) = call(
            &router,
            "CreateQueue",
            &create_queue(&cluster, &name, &["cap-cng-0"]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case queue {index} of {MAX_QUEUES_PER_CLUSTER}: expected 200, got {status} {body}"
        );
    }
    let (status, refused) = call(
        &router,
        "CreateQueue",
        &create_queue(&cluster, "cap-queue-over", &["cap-cng-0"]),
    )
    .await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case one queue past the ceiling: got {status} {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ServiceQuotaExceededException"),
        "case one queue past the ceiling: got {refused}"
    );
}

#[tokio::test]
async fn a_persisted_list_is_accepted_at_its_bound_and_refused_past_it() {
    let router = emulator();

    // Rows: the published bound and one past it, on a list the model itself does not
    // bound, so the ceiling is the one `crate::state::MAX_STORED_LIST_ENTRIES` sets.
    let cases: [(usize, u16); 2] = [
        (MAX_STORED_LIST_ENTRIES, 200),
        (MAX_STORED_LIST_ENTRIES + 1, 400),
    ];
    for (entries, expected) in cases {
        let subnets: Vec<String> = (0..entries)
            .map(|index| format!("subnet-{index:017}"))
            .collect();
        let mut request = create_cluster(&format!("list-cluster-{entries}"), "SMALL");
        request["networking"]["subnetIds"] = json!(subnets);
        let (status, body) = call(&router, "CreateCluster", &request).await;
        assert_eq!(
            status, expected,
            "case {entries} subnetIds: expected {expected}, got {status} {body}"
        );
        if expected == 200 {
            assert_eq!(
                body["cluster"]["networking"]["subnetIds"]
                    .as_array()
                    .map(Vec::len),
                Some(entries),
                "case {entries} subnetIds: expected every entry to be stored, got {body}"
            );
        } else {
            assert_eq!(
                body["__type"],
                json!("ValidationException"),
                "case {entries} subnetIds: got {body}"
            );
        }
    }
}
