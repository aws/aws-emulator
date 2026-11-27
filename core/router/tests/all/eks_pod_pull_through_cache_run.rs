// SPDX-License-Identifier: CC0-1.0

//! A pod whose image comes from an Amazon ECR PULL THROUGH CACHE rule runs its
//! container, and the pull that started it leaves the mirrored repository in the
//! registry.
//!
//! The RULE under test: **a pull through cache rule "provides a way to cache images from
//! an upstream registry source in your Amazon ECR private registry"
//! (`CreatePullThroughCacheRule`), so the FIRST pull of a reference under its
//! `ecrRepositoryPrefix` materialises that repository and the image in the private
//! registry — after which `DescribeRepositories` reports it — and the node's kubelet,
//! which makes that pull as the NODE'S IAM role, then creates the pod's container and
//! runs the program it declares. So the pod reports what that program did: `Succeeded`
//! for a one-shot program that exits with the status a completed program exits with,
//! with `containerStatuses[0].state.terminated` carrying the reason the runtime words
//! that status with.**
//!
//! Four crates have to agree for that to hold — Amazon EKS serves the Kubernetes API and
//! runs the pod's containers, Amazon EC2 Auto Scaling and Amazon EC2 own the instance the
//! node is, Amazon ECR holds the cache rule and performs the import, and IAM holds the
//! documents in force on the node role — so the suite lives here rather than in any one
//! of them.
//!
//! The rows differ along what the rule generalises over: the SPELLING the pod addresses
//! the registry by (the one AWS publishes for a private registry, and the one Amazon ECR
//! itself hands a client as the `proxyEndpoint` of its authorization token) and the
//! program the container declares (one that writes a line and one that computes a status
//! of its own). Every wait is for a CONDITION, re-read until it holds or a generous
//! deadline passes; nothing here measures elapsed time.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::eks_wire::{cluster_body, ec2, eks, iam, role_arn, Answer, REGION};
use crate::query_wire::elements;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// How often a clock-owed read is looked at again, and how many times in all: nothing
/// inside the emulator sleeps for a due instant, so a condition is polled rather than
/// waited for, with a deadline far above every span this emulator owes.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
/// See [`POLL_INTERVAL`]: 600 looks at 50ms is a 30-second deadline.
const POLLS: usize = 600;

/// The AWS-managed policies AWS documents for a node role that pulls from the account's
/// Amazon ECR registry.
const ECR_READ_POLICY: &str = "arn:aws:iam::aws:policy/AmazonEC2ContainerRegistryReadOnly";
const WORKER_NODE_POLICY: &str = "arn:aws:iam::aws:policy/AmazonEKSWorkerNodePolicy";

/// The rule's own members: the prefix the mirrored repositories are created under, and
/// the upstream registry it caches from.
const CACHE_PREFIX: &str = "upstream";
const UPSTREAM_REGISTRY_URL: &str = "public.ecr.aws";

/// The upstream repository the rows pull, and the tag they ask for — the path under the
/// rule's prefix the mirrored repository is created at.
const UPSTREAM_REPOSITORY: &str = "amazonlinux/amazonlinux";
const TAG: &str = "latest";

/// The container every pod of this suite declares.
const CONTAINER: &str = "app";

/// One answer from a cluster's published Kubernetes API address.
#[derive(Debug)]
struct ApiAnswer {
    status: u16,
    body: Value,
}

/// One request to that address, the way a kubeconfig-built client makes it: one HTTP/1.1
/// exchange on the port the cluster published, presenting the ACCOUNT's own credential —
/// the principal that created the cluster.
fn k8s(
    runtime: &tokio::runtime::Runtime,
    endpoint: &str,
    what: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> ApiAnswer {
    let authority = endpoint
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("{what}: expected an https endpoint, got {endpoint:?}"))
        .to_string();
    let (_, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("{what}: the endpoint must carry a port, got {authority:?}"));
    let port: u16 = port
        .parse()
        .unwrap_or_else(|error| panic!("{what}: {port:?} must be a port: {error}"));
    let payload = body.map(Value::to_string).unwrap_or_default();
    runtime.block_on(async move {
        let mut stream = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap_or_else(|error| panic!("{what}: the published address must accept: {error}"));
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n\
             {payload}",
            crate::eks_wire::account_kubernetes_bearer_token(),
            payload.len()
        );
        stream
            .write_all(request.as_bytes())
            .await
            .unwrap_or_else(|error| panic!("{what}: the request must be written: {error}"));
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .await
            .unwrap_or_else(|error| panic!("{what}: the answer must be read: {error}"));
        let text = String::from_utf8_lossy(&raw).to_string();
        let (head, answered) = text
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("{what}: expected an HTTP answer, got {text:?}"));
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .unwrap_or_else(|| panic!("{what}: expected a status line, got {head:?}"));
        ApiAnswer {
            status,
            body: serde_json::from_str(answered).unwrap_or(Value::Null),
        }
    })
}

/// One Amazon ECR call (`awsJson1_1`, `AmazonEC2ContainerRegistry_V20150921.<Op>`).
fn ecr(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: &Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ecr.{REGION}.amazonaws.com/"))
        .header("host", format!("ecr.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ecr/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerRegistry_V20150921.{operation}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let parsed = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer {
        status,
        error_type: None,
        text,
        body: parsed,
    }
}

/// Poll `read` until `holds` answers true, and answer the last read; panic naming `what`
/// when the deadline passes first.
fn read_until(
    what: &str,
    mut read: impl FnMut() -> ApiAnswer,
    holds: impl Fn(&ApiAnswer) -> bool,
) -> ApiAnswer {
    let mut last = read();
    for _ in 0..POLLS {
        if holds(&last) {
            return last;
        }
        std::thread::sleep(POLL_INTERVAL);
        last = read();
    }
    panic!("expected {what}; the last read answered {last:?}");
}

/// The account's own default subnets, read from EC2 rather than invented.
fn default_subnets(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime) -> Vec<String> {
    let defaults = ec2(
        router,
        runtime,
        "DescribeSubnets",
        &[
            ("Filter.1.Name", "default-for-az"),
            ("Filter.1.Value.1", "true"),
        ],
    );
    assert_eq!(
        defaults.status, 200,
        "setup: DescribeSubnets must succeed, got {defaults:?}"
    );
    let available = elements(&defaults.text, "subnetId");
    assert!(
        available.len() >= 2,
        "setup: the account must have default subnets, got {available:?}"
    );
    vec![available[0].clone(), available[1].clone()]
}

/// Create the node role, carrying the two AWS-managed policies a node that pulls from
/// the account's registry needs, and answer its ARN.
fn node_role(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime, name: &str) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "ec2.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }]
    })
    .to_string();
    let created = iam(
        router,
        runtime,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
    );
    assert_eq!(
        created.status, 200,
        "setup: the node role {name:?} must be created, got {created:?}"
    );
    for policy in [WORKER_NODE_POLICY, ECR_READ_POLICY] {
        let attached = iam(
            router,
            runtime,
            "AttachRolePolicy",
            &[("RoleName", name), ("PolicyArn", policy)],
        );
        assert_eq!(
            attached.status, 200,
            "setup: {policy} must attach to {name:?}, got {attached:?}"
        );
    }
    role_arn(name)
}

/// Create the cluster `name` with one node group running under `role`, wait for the node
/// group to be ACTIVE and for the cluster's API server to report one node, and answer the
/// address the cluster published.
fn cluster_with_a_node(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    role: &str,
    subnets: &[String],
) -> String {
    let created = eks(
        router,
        runtime,
        "POST",
        "/clusters",
        Some(cluster_body(name, subnets)),
    );
    assert_eq!(
        created.status, 200,
        "setup: the cluster {name:?} must be created, got {created:?} ({})",
        created.message()
    );
    let mut endpoint = String::new();
    for _ in 0..POLLS {
        let described = eks(router, runtime, "GET", &format!("/clusters/{name}"), None);
        if described.body["cluster"]["status"] == json!("ACTIVE") {
            if let Some(published) = described.body["cluster"]["endpoint"].as_str() {
                endpoint = published.to_string();
                break;
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    assert!(
        !endpoint.is_empty(),
        "setup: the cluster {name:?} must publish its API server address"
    );
    let nodegroup = eks(
        router,
        runtime,
        "POST",
        &format!("/clusters/{name}/node-groups"),
        Some(json!({
            "nodegroupName": "runners",
            "nodeRole": role,
            "subnets": subnets,
            "scalingConfig": { "minSize": 1, "maxSize": 1, "desiredSize": 1 },
        })),
    );
    assert_eq!(
        nodegroup.status, 200,
        "setup: the node group of {name:?} must be created, got {nodegroup:?} ({})",
        nodegroup.message()
    );
    for _ in 0..POLLS {
        let described = eks(
            router,
            runtime,
            "GET",
            &format!("/clusters/{name}/node-groups/runners"),
            None,
        );
        if described.body["nodegroup"]["status"] == json!("ACTIVE") {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    // The node the pods below are scheduled onto: the instance the node group launched,
    // as the cluster's own API server reports it.
    for _ in 0..POLLS {
        let nodes = k8s(
            runtime,
            &endpoint,
            "the cluster's nodes",
            "GET",
            "/api/v1/nodes",
            None,
        );
        if nodes.body["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            return endpoint;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    panic!("setup: the cluster {name:?} must report the node its node group launched");
}

/// The repository names `DescribeRepositories` reports for the account's registry.
fn described_repositories(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
) -> Vec<String> {
    let described = ecr(router, runtime, "DescribeRepositories", &json!({}));
    assert_eq!(
        described.status, 200,
        "DescribeRepositories must be served, got {described:?}"
    );
    described.body["repositories"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|repository| repository["repositoryName"].as_str().map(str::to_string))
        .collect()
}

/// One row: the pod, the registry host its image is addressed by, the program its
/// container declares and the status that program exits with.
struct Row {
    case: &'static str,
    pod: &'static str,
    registry: String,
    command: &'static [&'static str],
    exit_code: i64,
    phase: &'static str,
}

#[test]
fn a_pods_pull_through_cache_image_is_imported_and_its_container_runs_to_completion() {
    std::thread::Builder::new()
        .name("eks-pull-through-cache-run".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .expect("a current-thread runtime");

            // THE REGISTRY'S OWN RECORD: one pull through cache rule, and nothing under
            // its prefix yet.
            let rule = ecr(
                &router,
                &runtime,
                "CreatePullThroughCacheRule",
                &json!({
                    "ecrRepositoryPrefix": CACHE_PREFIX,
                    "upstreamRegistryUrl": UPSTREAM_REGISTRY_URL,
                }),
            );
            assert_eq!(
                rule.status, 200,
                "setup: the cache rule must be created, got {rule:?}"
            );
            let mirrored = format!("{CACHE_PREFIX}/{UPSTREAM_REPOSITORY}");
            assert!(
                !described_repositories(&router, &runtime).contains(&mirrored),
                "setup: the registry must hold no mirrored repository before any pull"
            );

            // The address Amazon ECR itself hands a client to pull with: "the endpoint of
            // the registry" an authorization token is for.
            let token = ecr(&router, &runtime, "GetAuthorizationToken", &json!({}));
            assert_eq!(
                token.status, 200,
                "setup: GetAuthorizationToken must be served, got {token:?}"
            );
            let proxy_endpoint = token.body["authorizationData"][0]["proxyEndpoint"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("setup: the token must report a proxyEndpoint, got {token:?}")
                })
                .to_string();
            let proxy_registry = proxy_endpoint
                .strip_prefix("https://")
                .unwrap_or(&proxy_endpoint)
                .trim_end_matches('/')
                .to_string();

            let subnets = default_subnets(&router, &runtime);
            let role = node_role(&router, &runtime, "eksCacheNodeRole");
            let endpoint =
                cluster_with_a_node(&router, &runtime, "cache-cluster", &role, &subnets);

            let account = engine::emulator_account_id();
            let rows = [
                Row {
                    case: "the published spelling of this account's private registry",
                    pod: "cache-published",
                    registry: format!("{account}.dkr.ecr.{REGION}.amazonaws.com"),
                    command: &["sh", "-c", "echo pulled-ok"],
                    exit_code: 0,
                    phase: "Succeeded",
                },
                Row {
                    case: "the proxyEndpoint Amazon ECR handed the client",
                    pod: "cache-proxy-endpoint",
                    registry: proxy_registry.clone(),
                    command: &["sh", "-c", "exit $(seq 2 | wc -l)"],
                    exit_code: 2,
                    phase: "Failed",
                },
            ];

            for row in &rows {
                let Row {
                    case,
                    pod,
                    registry,
                    command,
                    exit_code,
                    phase,
                } = row;
                let image = format!("{registry}/{CACHE_PREFIX}/{UPSTREAM_REPOSITORY}:{TAG}");
                let created = k8s(
                    &runtime,
                    &endpoint,
                    case,
                    "POST",
                    "/api/v1/namespaces/default/pods",
                    Some(&json!({
                        "metadata": { "name": pod },
                        "spec": {
                            "restartPolicy": "Never",
                            "containers": [{
                                "name": CONTAINER,
                                "image": image,
                                "command": command,
                            }],
                        },
                    })),
                );
                assert_eq!(
                    created.status, 201,
                    "case {case:?}: the pod must be created, got {created:?}"
                );

                let concluded = read_until(
                    "the pod to reach a terminal phase",
                    || {
                        k8s(
                            &runtime,
                            &endpoint,
                            case,
                            "GET",
                            &format!("/api/v1/namespaces/default/pods/{pod}"),
                            None,
                        )
                    },
                    |read| {
                        read.body["status"]["phase"] == json!("Succeeded")
                            || read.body["status"]["phase"] == json!("Failed")
                    },
                );
                assert_eq!(
                    concluded.body["status"]["phase"],
                    json!(phase),
                    "case {case:?}: the phase the container's own status decides, got {}",
                    concluded.body
                );
                let held = concluded.body["status"]["containerStatuses"][0].clone();
                let terminated = held["state"]["terminated"].clone();
                assert_eq!(
                    terminated["exitCode"],
                    json!(exit_code),
                    "case {case:?}: expected the status the program exited with, got {held}"
                );
                assert_eq!(
                    terminated["reason"],
                    json!(if *exit_code == 0 { "Completed" } else { "Error" }),
                    "case {case:?}: expected the reason the runtime words that status with, \
                     got {held}"
                );
                // THE PULL WAS REAL: the kubelet's pull of a reference under the rule's
                // prefix resolved to a manifest digest the registry now holds.
                let image_id = held["imageID"].as_str().unwrap_or_default().to_string();
                assert!(
                    image_id.contains("@sha256:"),
                    "case {case:?}: the container must report the digest the registry \
                     resolved, got {image_id:?}"
                );

                // AND THE IMPORT HAPPENED: the mirrored repository is one the registry's
                // own read reports, under the rule's prefix.
                let held_repositories = described_repositories(&router, &runtime);
                assert!(
                    held_repositories.contains(&mirrored),
                    "case {case:?}: the first pull must materialise {mirrored:?} in the \
                     private registry, got {held_repositories:?}"
                );
                let images = ecr(
                    &router,
                    &runtime,
                    "DescribeImages",
                    &json!({ "repositoryName": mirrored }),
                );
                assert_eq!(
                    images.status, 200,
                    "case {case:?}: the mirrored repository's images must be served, got \
                     {images:?}"
                );
                let tags: Vec<String> = images.body["imageDetails"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .flat_map(|detail| {
                        detail["imageTags"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|tag| tag.as_str().map(str::to_string))
                            .collect::<Vec<String>>()
                    })
                    .collect();
                assert!(
                    tags.contains(&TAG.to_string()),
                    "case {case:?}: the imported image must carry the tag the pull asked \
                     for, got {tags:?}"
                );
            }
        })
        .expect("the test thread must start")
        .join()
        .expect("the test body must not panic");
}
