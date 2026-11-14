// SPDX-License-Identifier: CC0-1.0

//! What a client reaches at the address a cluster publishes as its `endpoint`,
//! beyond the one object it creates there.
//!
//! The RULE under test: the Kubernetes API server a cluster publishes answers for
//! the RESOURCES that cluster holds — the discovery document a client resolves a
//! kind against before it addresses anything, the cluster's nodes, and the pods
//! it admits with the spec it admitted them with. `Cluster$endpoint` is "the
//! endpoint for your Kubernetes API server", so what answers there is the
//! Kubernetes API: a client lists the core group's resources, reads the cluster's
//! nodes, and creates a pod whose containers, their environment and their
//! resources it reads back — scheduled onto a node of that cluster when it has
//! one.
//!
//! Each suite asserts that over a TABLE whose rows differ along what the rule
//! generalises over: which resource of the discovery document is read and whether
//! it is namespaced; how many nodes the cluster's node group registered and
//! whether a node is read by name or not held at all; whether the pod declares
//! one container or several, what each one declares (an image, an environment, a
//! resource request, a command and a port) and which namespace it is created in;
//! whether the cluster has a node to schedule the pod onto at all; and the media
//! type a client writes its body in.
//!
//! The client is the raw, kubeconfig-built one `kubernetes_api.rs` dials with: a
//! TCP connection to the published address, TLS verified against nothing but the
//! published certificate authority, and one HTTP/1.1 exchange. The nodes a node
//! group registered are the instances the Amazon EC2 services launched for it, and
//! no Amazon EC2 service is mounted in this crate's harness — so a row that needs
//! registered nodes writes them into the same `NodegroupCapacity::nodes` the
//! node-group create fills from those services, through the crate's public `state`
//! module; the call under test is always a real request to the published address.

use std::time::{Duration, Instant};

use service_com_amazonaws_eks_awswesleyfrontend::state::kubernetes_objects::POD_START_DELAY;
use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ComAmazonawsEksAwswesleyfrontendState, NodeInstance,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::{create_cluster, str_member, REGION};
use crate::kubernetes_api::{dial, dial_as, kubeconfig_of, poll_until_active, ApiAnswer, Kubeconfig};
use crate::nodegroup_lifecycle::create_nodegroup_in;
use crate::pod_identity_associations::create_association;
use crate::wire::emulator_with_engine;

/// The IAM role an association in this suite binds a service account to.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/PodIdentityRole";

/// How long a polled condition is given before the case fails. Generous: the one
/// span these cases wait out is the service's own pod-start span, and nothing here
/// measures elapsed time.
const DEADLINE: Duration = Duration::from_secs(60);

/// A cluster that has finished provisioning, with the kubeconfig members a client
/// builds its connection from, and a handle on the engine behind the emulator.
async fn published_cluster(
    name: &str,
) -> (
    MultiServiceRouter,
    std::sync::Arc<engine::InMemoryEngine>,
    Kubeconfig,
) {
    let (router, engine) = emulator_with_engine();
    let created = create_cluster(&router, REGION, name).await;
    assert_eq!(
        created.status, 200,
        "cluster {name:?}: expected 200 from CreateCluster, got {created:?}"
    );
    let described = poll_until_active(&router, REGION, name).await;
    let config = kubeconfig_of(name, &described);
    (router, engine, config)
}

/// Register `nodes` instances on the node group `nodegroup` of `cluster`, the way
/// the Amazon EC2 services report the instances its Auto Scaling group launched.
fn register_nodes(
    engine: &engine::InMemoryEngine,
    cluster: &str,
    nodegroup: &str,
    nodes: &[NodeInstance],
) {
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let mut guard = store.lock();
    guard.materialize_due_transitions(std::time::SystemTime::now());
    let record = guard
        .nodegroups
        .get_mut(&(
            REGION.to_string(),
            cluster.to_string(),
            nodegroup.to_string(),
        ))
        .expect("setup: the node group was created above");
    record.capacity.nodes = nodes.to_vec();
}

/// One instance of a node group, as the Auto Scaling group and Amazon EC2 report
/// one.
fn instance(id: &str, zone: &str, instance_type: &str, address: &str) -> NodeInstance {
    NodeInstance {
        instance_id: id.to_string(),
        private_dns_name: format!("ip-{}.{REGION}.compute.internal", address.replace('.', "-")),
        availability_zone: zone.to_string(),
        instance_type: instance_type.to_string(),
        private_ip_address: address.to_string(),
    }
}

/// Create the node group `name` of `cluster` and wait for it to report `ACTIVE`,
/// which is when its nodes have joined the cluster.
async fn active_nodegroup(router: &MultiServiceRouter, cluster: &str, name: &str) {
    let created = create_nodegroup_in(
        router,
        REGION,
        cluster,
        &serde_json::json!({
            "nodegroupName": name,
            "subnets": ["subnet-aaaa1111", "subnet-bbbb2222"],
            "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "node group {name:?}: expected 200 from CreateNodegroup, got {created:?}"
    );
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = crate::wire::call(
            router,
            REGION,
            "GET",
            &format!(
                "/clusters/{}/node-groups/{}",
                crate::wire::encode_label(cluster),
                crate::wire::encode_label(name)
            ),
            "",
        )
        .await;
        if described.body["nodegroup"]["status"] == serde_json::json!("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "node group {name:?}: never reported ACTIVE; the last answer was {described:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The resources of the core API group a client resolves a kind against.
#[tokio::test]
async fn the_core_api_group_lists_the_resources_the_server_serves() {
    let (_router, _engine, config) = published_cluster("discovery-cluster").await;

    let versions = dial(&config, "the core group", "GET", "/api", None).await;
    assert_eq!(
        versions.status, 200,
        "the core group's versions must be served, got {versions:?}"
    );
    assert_eq!(
        versions.body["versions"],
        serde_json::json!(["v1"]),
        "the core group serves v1, got {versions:?}"
    );

    let listed = dial(&config, "the core group's resources", "GET", "/api/v1", None).await;
    assert_eq!(
        listed.status, 200,
        "the core group's resource list must be served, got {listed:?}"
    );
    assert_eq!(
        listed.body["kind"],
        serde_json::json!("APIResourceList"),
        "the resource list is an APIResourceList, got {listed:?}"
    );

    // (the resource a client addresses, the kind it carries, whether it lives in a
    // namespace, a verb it must serve) — one row per resource this server serves.
    let rows = [
        ("namespaces", "Namespace", false, "create"),
        ("nodes", "Node", false, "list"),
        ("pods", "Pod", true, "create"),
        ("serviceaccounts", "ServiceAccount", true, "create"),
        ("serviceaccounts/token", "TokenRequest", true, "create"),
    ];
    let resources = listed.body["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("the resource list carries resources, got {listed:?}"))
        .clone();
    for (name, kind, namespaced, verb) in rows {
        let found = resources
            .iter()
            .find(|resource| resource["name"] == serde_json::json!(name))
            .unwrap_or_else(|| {
                panic!("case {name:?}: the resource list must name it, got {resources:?}")
            });
        assert_eq!(
            found["kind"],
            serde_json::json!(kind),
            "case {name:?}: expected the kind {kind:?}, got {found:?}"
        );
        assert_eq!(
            found["namespaced"],
            serde_json::json!(namespaced),
            "case {name:?}: expected namespaced {namespaced}, got {found:?}"
        );
        let verbs = found["verbs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: expected verbs, got {found:?}"));
        assert!(
            verbs.contains(&serde_json::json!(verb)),
            "case {name:?}: expected the verb {verb:?} among {verbs:?}"
        );
    }
}

/// The cluster's nodes are the instances its node groups registered, and nothing
/// else.
#[tokio::test]
async fn the_cluster_reports_the_nodes_its_node_groups_registered() {
    let cluster = "nodes-cluster";
    let (router, engine, config) = published_cluster(cluster).await;

    // A cluster with no node group has no node, which is what a client reads
    // rather than a node no service holds.
    let empty = dial(&config, "a cluster with no node group", "GET", "/api/v1/nodes", None).await;
    assert_eq!(
        empty.status, 200,
        "case \"no node group\": the nodes must be served, got {empty:?}"
    );
    assert_eq!(
        empty.body["kind"],
        serde_json::json!("NodeList"),
        "case \"no node group\": a listing is a NodeList, got {empty:?}"
    );
    assert_eq!(
        empty.body["items"],
        serde_json::json!([]),
        "case \"no node group\": a cluster with no node group reports no node, got {empty:?}"
    );

    // (the case, the instances the node group registered) — the rule holds for one
    // instance and for several, whatever each launched as and wherever it launched.
    let registered = [
        instance("i-0aaa1111aaa1111aa", "us-west-2a", "t3.medium", "10.0.1.11"),
        instance("i-0bbb2222bbb2222bb", "us-west-2b", "m5.large", "10.0.2.22"),
    ];
    active_nodegroup(&router, cluster, "workers").await;
    register_nodes(&engine, cluster, "workers", &registered);

    let listed = dial(&config, "a node group with instances", "GET", "/api/v1/nodes", None).await;
    let items = listed.body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"registered\": expected items, got {listed:?}"))
        .clone();
    assert_eq!(
        items.len(),
        registered.len(),
        "case \"registered\": one node per registered instance, got {listed:?}"
    );
    for expected in &registered {
        let name = expected.node_name();
        let node = items
            .iter()
            .find(|node| node["metadata"]["name"] == serde_json::json!(name))
            .unwrap_or_else(|| panic!("case {name:?}: the node must be reported, got {items:?}"));
        assert_eq!(
            node["spec"]["providerID"],
            serde_json::json!(format!(
                "aws:///{}/{}",
                expected.availability_zone, expected.instance_id
            )),
            "case {name:?}: the node names the instance behind it, got {node:?}"
        );
        assert_eq!(
            node["metadata"]["labels"]["node.kubernetes.io/instance-type"],
            serde_json::json!(expected.instance_type),
            "case {name:?}: the node reports the type it launched as, got {node:?}"
        );
        assert_eq!(
            node["metadata"]["labels"]["topology.kubernetes.io/zone"],
            serde_json::json!(expected.availability_zone),
            "case {name:?}: the node reports the zone it launched in, got {node:?}"
        );
        assert_eq!(
            node["metadata"]["labels"]["eks.amazonaws.com/nodegroup"],
            serde_json::json!("workers"),
            "case {name:?}: the node names the node group it belongs to, got {node:?}"
        );
        let ready = node["status"]["conditions"]
            .as_array()
            .and_then(|conditions| {
                conditions
                    .iter()
                    .find(|condition| condition["type"] == serde_json::json!("Ready"))
                    .cloned()
            })
            .unwrap_or_else(|| panic!("case {name:?}: expected a Ready condition, got {node:?}"));
        assert_eq!(
            ready["status"],
            serde_json::json!("True"),
            "case {name:?}: a node of an ACTIVE node group has joined, got {ready:?}"
        );

        // And it is readable on its own path, which is how a client addresses one.
        let read = dial(
            &config,
            name,
            "GET",
            &format!("/api/v1/nodes/{name}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: the node must be readable by name, got {read:?}"
        );
        assert_eq!(
            read.body["metadata"]["uid"], node["metadata"]["uid"],
            "case {name:?}: the node reports the same uid on both paths, got {read:?}"
        );
    }

    // A name no instance of the cluster carries is not a node of it.
    let absent = dial(
        &config,
        "a node the cluster does not hold",
        "GET",
        "/api/v1/nodes/ip-10-0-9-99.us-west-2.compute.internal",
        None,
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case \"absent\": a node the cluster does not hold is not found, got {absent:?}"
    );
    assert_eq!(
        absent.body["reason"],
        serde_json::json!("NotFound"),
        "case \"absent\": the refusal is the Kubernetes not-found Status, got {absent:?}"
    );
}

/// A pod is read back with the SPEC it was admitted with — every container the
/// manifest declared, with its image, its environment and its resources — and
/// with the service account the API server resolved for it.
#[tokio::test]
async fn an_admitted_pod_carries_the_spec_it_was_admitted_with() {
    let cluster = "pod-admission-cluster";
    let (router, _engine, config) = published_cluster(cluster).await;

    // The cluster holds an EKS Pod Identity association for the `default` service
    // account of the `default` namespace: a pod of that service account is
    // admitted exactly like any other, so the two namespaces' rows below must
    // read back alike.
    let associated = create_association(
        &router,
        REGION,
        cluster,
        &serde_json::json!({
            "namespace": "default",
            "serviceAccount": "default",
            "roleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        associated.status, 200,
        "the association must be created, got {associated:?}"
    );

    /// One pod create: the namespace it is created in, the manifest it carries,
    /// and how many containers that manifest declares.
    struct PodCase {
        label: &'static str,
        namespace: &'static str,
        manifest: serde_json::Value,
        containers: usize,
    }
    let cases = vec![
        PodCase {
            label: "one container, with a resource request",
            namespace: "default",
            manifest: serde_json::json!({
                "metadata": { "name": "one-container" },
                "spec": {
                    "containers": [{
                        "name": "app",
                        "image": "public.ecr.aws/amazonlinux/amazonlinux:2023",
                        "resources": { "requests": { "cpu": "200m" } },
                    }],
                },
            }),
            containers: 1,
        },
        PodCase {
            label: "several containers, one with its own environment",
            namespace: "default",
            manifest: serde_json::json!({
                "metadata": { "name": "many-containers" },
                "spec": {
                    "containers": [
                        { "name": "app", "image": "nginx:1.27" },
                        {
                            "name": "sidecar",
                            "image": "busybox:1.36",
                            "env": [{ "name": "SIDECAR", "value": "yes" }],
                        },
                    ],
                },
            }),
            containers: 2,
        },
        PodCase {
            label: "a container with a command and a port",
            namespace: "default",
            manifest: serde_json::json!({
                "metadata": { "name": "with-command" },
                "spec": {
                    "containers": [{
                        "name": "app",
                        "image": "busybox:1.36",
                        "command": ["/bin/sh", "-c", "sleep 3600"],
                        "ports": [{ "containerPort": 8080 }],
                    }],
                },
            }),
            containers: 1,
        },
        PodCase {
            label: "a namespace the cluster binds no service account in",
            namespace: "kube-public",
            manifest: serde_json::json!({
                "metadata": { "name": "other-namespace" },
                "spec": { "containers": [{ "name": "app", "image": "nginx:1.27" }] },
            }),
            containers: 1,
        },
    ];

    for case in &cases {
        let label = case.label;
        let created = dial(
            &config,
            label,
            "POST",
            &format!("/api/v1/namespaces/{}/pods", case.namespace),
            Some(&case.manifest),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected 201 from the pod create, got {created:?}"
        );
        let name = str_member(label, "metadata.name", &created.body["metadata"]["name"]);
        let containers = created.body["spec"]["containers"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {label:?}: the admitted pod carries its containers, got {created:?}")
            })
            .clone();
        assert_eq!(
            containers.len(),
            case.containers,
            "case {label:?}: every container the manifest declared is admitted, got {created:?}"
        );
        // Every container reads back with what the manifest declared for it, and
        // the cluster adds nothing to it.
        for (admitted, declared) in containers.iter().zip(
            case.manifest["spec"]["containers"]
                .as_array()
                .unwrap_or(&Vec::new()),
        ) {
            assert_eq!(
                admitted, declared,
                "case {label:?}: the admitted container is the declared one, got {admitted:?}"
            );
        }
        // And the service account the API server resolved is reported under both
        // members Kubernetes reports it under.
        assert_eq!(
            created.body["spec"]["serviceAccountName"],
            serde_json::json!("default"),
            "case {label:?}: the resolved service account is reported, got {created:?}"
        );
        assert_eq!(
            created.body["spec"]["serviceAccount"],
            created.body["spec"]["serviceAccountName"],
            "case {label:?}: both service-account members agree, got {created:?}"
        );

        // And a read of the pod reports the same spec the create admitted.
        let read = dial(
            &config,
            label,
            "GET",
            &format!("/api/v1/namespaces/{}/pods/{name}", case.namespace),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the pod must be readable, got {read:?}"
        );
        assert_eq!(
            read.body["spec"], created.body["spec"],
            "case {label:?}: the spec a read reports is the one the create admitted"
        );
    }
}

/// A pod is scheduled onto a node of the cluster, and reaches the phase a started
/// pod is in — and a cluster with no node has nowhere to put it.
#[tokio::test]
async fn a_pod_is_scheduled_onto_a_node_of_the_cluster_and_then_runs() {
    let cluster = "pod-scheduling-cluster";
    let (router, engine, config) = published_cluster(cluster).await;

    // With no node group there is no node, so the pod is admitted and stays
    // unscheduled, with the reason Kubernetes gives.
    let unschedulable = create_pod(&config, "a cluster with no node", "no-node").await;
    assert_eq!(
        unschedulable.body["spec"]["nodeName"],
        serde_json::Value::Null,
        "case \"no node\": a cluster with no node binds the pod to nothing, got {unschedulable:?}"
    );
    let scheduled_condition = |answer: &ApiAnswer| {
        answer.body["status"]["conditions"]
            .as_array()
            .and_then(|conditions| {
                conditions
                    .iter()
                    .find(|condition| condition["type"] == serde_json::json!("PodScheduled"))
                    .cloned()
            })
            .unwrap_or(serde_json::Value::Null)
    };
    let condition = scheduled_condition(&unschedulable);
    assert_eq!(
        condition["status"],
        serde_json::json!("False"),
        "case \"no node\": the pod is not scheduled, got {condition:?}"
    );
    assert_eq!(
        condition["reason"],
        serde_json::json!("Unschedulable"),
        "case \"no node\": the reason is the one Kubernetes gives, got {condition:?}"
    );
    assert_eq!(
        unschedulable.body["status"]["phase"],
        serde_json::json!("Pending"),
        "case \"no node\": a pod no node took has not started, got {unschedulable:?}"
    );

    // With a node group whose instances have joined, the pod is bound to one of
    // its nodes and its containers are owed their start.
    active_nodegroup(&router, cluster, "workers").await;
    let node = instance("i-0ccc3333ccc3333cc", "us-west-2c", "t3.large", "10.0.3.33");
    register_nodes(&engine, cluster, "workers", std::slice::from_ref(&node));

    let created = create_pod(&config, "a cluster with a node", "on-node").await;
    assert_eq!(
        created.body["spec"]["nodeName"],
        serde_json::json!(node.node_name()),
        "case \"a node\": the pod is bound to the cluster's node, got {created:?}"
    );
    assert_eq!(
        scheduled_condition(&created)["status"],
        serde_json::json!("True"),
        "case \"a node\": the pod is scheduled, got {created:?}"
    );
    assert_eq!(
        created.body["status"]["phase"],
        serde_json::json!("Pending"),
        "case \"a node\": a pod reads Pending until its containers start, got {created:?}"
    );

    // And it reaches the phase a started pod is in, with its one container ready.
    // The wait is for the CONDITION, re-read until it holds.
    let deadline = Instant::now() + DEADLINE;
    let running = loop {
        let read = dial(
            &config,
            "a cluster with a node",
            "GET",
            "/api/v1/namespaces/default/pods/on-node",
            None,
        )
        .await;
        if read.body["status"]["phase"] == serde_json::json!("Running") {
            break read;
        }
        assert!(
            Instant::now() < deadline,
            "case \"a node\": the pod never reached Running; the last answer was {read:?}"
        );
        tokio::time::sleep(POD_START_DELAY / 4).await;
    };
    let statuses = running.body["status"]["containerStatuses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        statuses.len(),
        1,
        "case \"a node\": one status per container, got {running:?}"
    );
    assert_eq!(
        statuses[0]["ready"],
        serde_json::json!(true),
        "case \"a node\": a started container is ready, got {statuses:?}"
    );
    assert!(
        statuses[0]["state"]["running"]["startedAt"].is_string(),
        "case \"a node\": a started container reports when it started, got {statuses:?}"
    );

    // And the pod that was admitted while the cluster had no node is bound as soon
    // as one is there, rather than staying unscheduled for ever: the scheduler
    // takes a pending pod the moment there is capacity for it.
    let deadline = Instant::now() + DEADLINE;
    let late = loop {
        let read = dial(
            &config,
            "a pod admitted before the node",
            "GET",
            "/api/v1/namespaces/default/pods/no-node",
            None,
        )
        .await;
        if read.body["status"]["phase"] == serde_json::json!("Running") {
            break read;
        }
        assert!(
            Instant::now() < deadline,
            "case \"admitted first\": the pod never reached Running once a node was there; \
             the last answer was {read:?}"
        );
        tokio::time::sleep(POD_START_DELAY / 4).await;
    };
    assert_eq!(
        late.body["spec"]["nodeName"],
        serde_json::json!(node.node_name()),
        "case \"admitted first\": the pod is bound to the node that appeared, got {late:?}"
    );
    assert_eq!(
        scheduled_condition(&late)["status"],
        serde_json::json!("True"),
        "case \"admitted first\": the pod reports itself scheduled, got {late:?}"
    );

    // A pod that NAMES a node the cluster does not hold is bound to nothing: the
    // name is not capacity, so the pod waits like any other unschedulable one.
    let invented = dial(
        &config,
        "a pod naming a node the cluster does not hold",
        "POST",
        "/api/v1/namespaces/default/pods",
        Some(&serde_json::json!({
            "metadata": { "name": "named-absent-node" },
            "spec": {
                "nodeName": "ip-10-0-9-99.us-west-2.compute.internal",
                "containers": [{ "name": "app", "image": "nginx:1.27" }],
            },
        })),
    )
    .await;
    assert_eq!(
        invented.status, 201,
        "case \"named absent node\": the pod is admitted, got {invented:?}"
    );
    assert_eq!(
        scheduled_condition(&invented)["status"],
        serde_json::json!("False"),
        "case \"named absent node\": a name no node carries binds nothing, got {invented:?}"
    );
    assert_eq!(
        invented.body["status"]["phase"],
        serde_json::json!("Pending"),
        "case \"named absent node\": the pod has not started, got {invented:?}"
    );

    // And a pod whose node the cluster STOPS holding is unbound again rather than
    // reporting a workload on capacity that is gone: deleting the node group takes
    // its nodes away.
    let deleted = crate::wire::call(
        &router,
        REGION,
        "DELETE",
        &format!(
            "/clusters/{}/node-groups/{}",
            crate::wire::encode_label(cluster),
            crate::wire::encode_label("workers")
        ),
        "",
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case \"the node group goes\": expected 200 from DeleteNodegroup, got {deleted:?}"
    );
    let deadline = Instant::now() + DEADLINE;
    let stranded = loop {
        let read = dial(
            &config,
            "the node group goes",
            "GET",
            "/api/v1/namespaces/default/pods/on-node",
            None,
        )
        .await;
        if read.body["status"]["phase"] == serde_json::json!("Pending") {
            break read;
        }
        assert!(
            Instant::now() < deadline,
            "case \"the node group goes\": the pod kept reporting a started workload; the \
             last answer was {read:?}"
        );
        tokio::time::sleep(POD_START_DELAY / 4).await;
    };
    assert_eq!(
        stranded.body["spec"]["nodeName"],
        serde_json::Value::Null,
        "case \"the node group goes\": the pod is bound to nothing, got {stranded:?}"
    );
    assert_eq!(
        scheduled_condition(&stranded)["status"],
        serde_json::json!("False"),
        "case \"the node group goes\": the pod is not scheduled, got {stranded:?}"
    );
}

/// A body in a media type this API server does not read is refused as one,
/// whichever resource it is addressed to.
#[tokio::test]
async fn a_body_in_a_media_type_the_server_does_not_read_is_refused() {
    let (_router, _engine, config) = published_cluster("media-type-cluster").await;
    let manifest = serde_json::json!({ "metadata": { "name": "unreadable" } });

    // (the case, the media type the client states, the resource it writes to) —
    // the rule holds for every media type this server does not read and for every
    // resource a client writes, and the one it does read is unaffected.
    let cases = [
        (
            "the Kubernetes protobuf media type",
            "application/vnd.kubernetes.protobuf",
            "/api/v1/namespaces",
            415,
        ),
        (
            "a media type nothing writes an object in",
            "application/octet-stream",
            "/api/v1/namespaces/default/pods",
            415,
        ),
        (
            "the media type the server reads, with its charset stated",
            "application/json; charset=utf-8",
            "/api/v1/namespaces",
            201,
        ),
    ];
    for (label, media_type, path, status) in cases {
        let answered = dial_as(&config, label, "POST", path, media_type, Some(&manifest)).await;
        assert_eq!(
            answered.status, status,
            "case {label:?}: expected HTTP {status}, got {answered:?}"
        );
        if status == 415 {
            assert_eq!(
                answered.body["reason"],
                serde_json::json!("UnsupportedMediaType"),
                "case {label:?}: the refusal names the body's format, got {answered:?}"
            );
            assert!(
                answered
                    .body
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|message| message.contains("application/json")),
                "case {label:?}: the refusal names what the server does read, got {answered:?}"
            );
        }
    }
}

/// A pod with one container, created in the `default` namespace.
async fn create_pod(config: &Kubeconfig, case: &str, name: &str) -> ApiAnswer {
    let created = dial(
        config,
        case,
        "POST",
        "/api/v1/namespaces/default/pods",
        Some(&serde_json::json!({
            "metadata": { "name": name },
            "spec": { "containers": [{ "name": "app", "image": "nginx:1.27" }] },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected 201 from the pod create, got {created:?}"
    );
    created
}
