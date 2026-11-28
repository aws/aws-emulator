// SPDX-License-Identifier: CC0-1.0

//! What a pod on AWS FARGATE capacity WRITES is delivered to CloudWatch Logs by the log
//! router the cluster's `aws-logging` ConfigMap configures, as the profile's Pod execution
//! role.
//!
//! The RULE under test: **a pod a Fargate profile selects runs beside a log router whose
//! configuration is the `aws-logging` ConfigMap of the `aws-observability` namespace. An
//! `[OUTPUT]` section of that configuration naming the CloudWatch Logs plugin names a
//! destination in that service — the `log_group_name` group, a stream named from
//! `log_stream_prefix` (the record's tag appended to it) or from the fixed
//! `log_stream_name`, with `auto_create_group` saying whether the router creates the group
//! — so every line each of that pod's containers wrote becomes one log event in that group,
//! in the stream of the container that wrote it, for every output whose `Match` takes that
//! container's records. The delivery is made AS the profile's Pod execution role ("the
//! Amazon Resource Name (ARN) of the Pod execution role to use for any Pod that matches the
//! selectors in the Fargate profile", `FargateProfile$podExecutionRoleArn`), so the group
//! creation, the stream creation and the put are each decided by the policies in force on
//! that role: a role no policy grants them to leaves the group with no stream and no event
//! in it, and the same role once granted delivers.**
//!
//! The rows differ along what the rule generalises over: how the stream is named (a prefix
//! the tag completes, and a fixed stream name), who provisions the destination group (the
//! router under `auto_create_group`, the account itself, and nobody at all), whether the
//! output's `Match` takes the pod's records, how many containers of one pod write (each with
//! its own stream) and how many lines they write, and the Pod execution role the delivery is
//! made as (granted the router's calls, granted the two creations but not the put, withheld
//! them, and withheld then granted). Both of the first two roles carry the AWS-managed policy
//! AWS publishes for a Fargate Pod execution role, so what differs between them is only the
//! grant for the router's own calls.
//!
//! Three crates have to agree for it to hold — Amazon EKS serves the Kubernetes API, places
//! the pod on its profile's capacity and runs its containers, CloudWatch Logs holds the
//! groups, the streams and the events, and IAM holds the documents in force on the Pod
//! execution role — so the suite lives here rather than in any one of them.
//!
//! Every wait is for a CONDITION, looked at again until it holds or a generous deadline
//! passes; nothing here measures elapsed time. The cases that expect a delivery make NO
//! Kubernetes API call after creating the pod: a pod's containers run because the pod was
//! admitted, so the lines they wrote are delivered whether or not a client reads the pod
//! again.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::eks_wire::{cluster_body, ec2, eks, iam, role_arn, Answer, REGION};
use crate::query_wire::elements;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// How often a clock-owed condition is looked at again, and how many times in all: nothing
/// inside the emulator sleeps for a due instant, so a condition is polled rather than waited
/// for, with a deadline far above every span this emulator owes.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
/// See [`POLL_INTERVAL`]: 600 looks at 50ms is a 30-second deadline.
const POLLS: usize = 600;

/// The `X-Amz-Target` prefix CloudWatch Logs names an operation under: its service shape's
/// name.
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// The namespace the profiles of this suite select, and the namespace the router's
/// configuration lives in.
const NAMESPACE: &str = "default";
/// The namespace Amazon EKS reads the Fargate log router's configuration from.
const OBSERVABILITY_NAMESPACE: &str = "aws-observability";
/// The name of the ConfigMap in it that carries that configuration.
const LOGGING_CONFIG_MAP: &str = "aws-logging";

/// The Fargate profile every cluster of this suite carries.
const PROFILE: &str = "log-routed-pods";

/// The image every pod here runs: a reference no registry this emulator holds answers for,
/// so nothing is decided about the pull and what is under test is where what the containers
/// wrote was delivered.
const IMAGE: &str = "registry.example.com/probe:1";

/// The stream prefix the prefix-named outputs configure, as the Fargate logging
/// documentation's own example spells it.
const STREAM_PREFIX: &str = "from-fluent-bit-";

/// The actions the log router performs under the Pod execution role, as IAM publishes them
/// for CloudWatch Logs — the grant AWS documents an operator adds to that role for a
/// Fargate pod's logging.
const ROUTER_ACTIONS: [&str; 4] = [
    "logs:CreateLogGroup",
    "logs:CreateLogStream",
    "logs:DescribeLogStreams",
    "logs:PutLogEvents",
];

/// The AWS-managed policy AWS publishes for a Fargate Pod execution role, which every such
/// role carries — and which grants the Amazon ECR pull and nothing else, so it is not what
/// lets the log router write.
const POD_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/AmazonEKSFargatePodExecutionRolePolicy";

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

/// One CloudWatch Logs call (`awsJson1_1`, `Logs_20140328.<Op>`), with the status it
/// answered — a client's own read of what that service holds.
fn logs(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: &Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://logs.{REGION}.amazonaws.com/"))
        .header("host", format!("logs.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/logs/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{LOGS_TARGET_PREFIX}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer {
        status,
        error_type: None,
        text,
        body,
    }
}

/// Whether CloudWatch Logs holds the group `group`, as a client finds out:
/// `DescribeLogGroups` narrowed to that name.
fn holds_group(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
) -> bool {
    let answered = logs(
        router,
        runtime,
        "DescribeLogGroups",
        &json!({ "logGroupNamePrefix": group }),
    );
    assert_eq!(
        answered.status, 200,
        "DescribeLogGroups must be served, got {answered:?}"
    );
    answered.body["logGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .any(|held| held["logGroupName"] == json!(group))
}

/// The streams `group` holds, by name — empty for a group CloudWatch Logs does not hold at
/// all, which is a destination nothing was delivered to.
fn streams_of(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
) -> Vec<String> {
    if !holds_group(router, runtime, group) {
        return Vec::new();
    }
    let answered = logs(
        router,
        runtime,
        "DescribeLogStreams",
        &json!({ "logGroupName": group }),
    );
    assert_eq!(
        answered.status, 200,
        "DescribeLogStreams on {group:?} must be served, got {answered:?}"
    );
    answered.body["logStreams"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|stream| stream["logStreamName"].as_str().map(str::to_string))
        .collect()
}

/// The messages one stream of `group` holds, in the order that service reports them.
fn events_of(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
    stream: &str,
) -> Vec<String> {
    let answered = logs(
        router,
        runtime,
        "GetLogEvents",
        &json!({ "logGroupName": group, "logStreamName": stream }),
    );
    assert_eq!(
        answered.status, 200,
        "GetLogEvents on {group:?}/{stream:?} must be served, got {answered:?}"
    );
    answered.body["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|event| event["message"].as_str().map(str::to_string))
        .collect()
}

/// The streams of `group` once at least `count` of them are there, or whatever is there when
/// the deadline passes — the delivery is owed at a stored instant, so the condition is
/// looked at again rather than waited for.
///
/// The pause between looks is awaited ON THE EMULATOR'S OWN RUNTIME, which is what lets the
/// work the cluster's clock owes its pods run while this suite is looking: in this harness
/// that runtime is driven only while a call is in flight.
fn streams_until(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
    count: usize,
) -> Vec<String> {
    let mut held = streams_of(router, runtime, group);
    for _ in 0..POLLS {
        if held.len() >= count {
            return held;
        }
        runtime.block_on(async { tokio::time::sleep(POLL_INTERVAL).await });
        held = streams_of(router, runtime, group);
    }
    held
}

/// The messages one stream of `group` holds once at least `count` of them are there, or
/// whatever it holds when the deadline passes — the put is owed at a stored instant, and a
/// stream the router has just opened is one it has not finished putting into yet.
///
/// The pause between looks is awaited on the emulator's own runtime, for the reason
/// [`streams_until`] gives.
fn events_until(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
    stream: &str,
    count: usize,
) -> Vec<String> {
    let mut held = events_of(router, runtime, group, stream);
    for _ in 0..POLLS {
        if held.len() >= count {
            return held;
        }
        runtime.block_on(async { tokio::time::sleep(POLL_INTERVAL).await });
        held = events_of(router, runtime, group, stream);
    }
    held
}

/// Poll `read` until `holds` answers true, and answer the last read; panic naming `what`
/// when the deadline passes first.
fn read_until(
    what: &str,
    mut read: impl FnMut() -> Answer,
    holds: impl Fn(&Answer) -> bool,
) -> Answer {
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

/// Create the IAM role `name` with the trust policy a Fargate Pod execution role carries —
/// the Fargate Pod plane's own service principal — carrying the AWS-managed policy AWS
/// publishes for such a role, and answer its ARN.
///
/// `grants` is the inline policy added beside it, naming the actions the role is granted on
/// every resource, and `None` for a role that carries the published policy alone — which is
/// the role AWS's own Fargate documentation prescribes, and which grants no CloudWatch Logs
/// call at all.
fn pod_execution_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    grants: Option<&[&str]>,
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "eks-fargate-pods.amazonaws.com" },
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
        "setup: the Pod execution role {name:?} must be created, got {created:?}"
    );
    let attached = iam(
        router,
        runtime,
        "AttachRolePolicy",
        &[("RoleName", name), ("PolicyArn", POD_EXECUTION_POLICY)],
    );
    assert_eq!(
        attached.status, 200,
        "setup: {POD_EXECUTION_POLICY} must attach to {name:?}, got {attached:?}"
    );
    if let Some(actions) = grants {
        grant(router, runtime, name, actions);
    }
    role_arn(name)
}

/// Put the inline policy allowing `actions` on every resource onto the role `name`, which is
/// how a role is granted (or re-granted) the calls a log router makes under it.
fn grant(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    actions: &[&str],
) {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }],
    })
    .to_string();
    let put = iam(
        router,
        runtime,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "podExecutionGrant"),
            ("PolicyDocument", &document),
        ],
    );
    assert_eq!(
        put.status, 200,
        "setup: the inline policy of {name:?} must be stored, got {put:?}"
    );
}

/// Create the cluster `name` whose ONLY capacity is a Fargate profile selecting
/// [`NAMESPACE`] and running under `role`, wait for both to be reported ready, and answer
/// the address the cluster published.
fn cluster_with_fargate_capacity(
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
    let active = read_until(
        "the cluster to reach ACTIVE with its API server address published",
        || eks(router, runtime, "GET", &format!("/clusters/{name}"), None),
        |answer| {
            answer.body["cluster"]["status"] == json!("ACTIVE")
                && answer.body["cluster"]["endpoint"].is_string()
        },
    );
    let endpoint = active.body["cluster"]["endpoint"]
        .as_str()
        .expect("the published API server address")
        .to_string();
    let profile = eks(
        router,
        runtime,
        "POST",
        &format!("/clusters/{name}/fargate-profiles"),
        Some(json!({
            "fargateProfileName": PROFILE,
            "podExecutionRoleArn": role,
            "selectors": [{ "namespace": NAMESPACE }],
        })),
    );
    assert_eq!(
        profile.status, 200,
        "setup: the Fargate profile of {name:?} must be created, got {profile:?} ({})",
        profile.message()
    );
    read_until(
        "the Fargate profile to reach ACTIVE",
        || {
            eks(
                router,
                runtime,
                "GET",
                &format!("/clusters/{name}/fargate-profiles/{PROFILE}"),
                None,
            )
        },
        |answer| answer.body["fargateProfile"]["status"] == json!("ACTIVE"),
    );
    endpoint
}

/// Create the `aws-observability` namespace of the cluster at `endpoint` and store
/// `output_conf` as the `output.conf` of its `aws-logging` ConfigMap — the configuration
/// surface Amazon EKS publishes for the Fargate log router.
fn configure_router(
    runtime: &tokio::runtime::Runtime,
    endpoint: &str,
    what: &str,
    output_conf: &str,
) {
    let namespace = k8s(
        runtime,
        endpoint,
        what,
        "POST",
        "/api/v1/namespaces",
        Some(&json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": { "name": OBSERVABILITY_NAMESPACE },
        })),
    );
    assert_eq!(
        namespace.status, 201,
        "{what}: the {OBSERVABILITY_NAMESPACE} namespace must be created, got {namespace:?}"
    );
    let configured = k8s(
        runtime,
        endpoint,
        what,
        "POST",
        &format!("/api/v1/namespaces/{OBSERVABILITY_NAMESPACE}/configmaps"),
        Some(&json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": { "name": LOGGING_CONFIG_MAP },
            "data": { "output.conf": output_conf },
        })),
    );
    assert_eq!(
        configured.status, 201,
        "{what}: the {LOGGING_CONFIG_MAP} ConfigMap must be created, got {configured:?}"
    );
}

/// Create the pod `pod` in [`NAMESPACE`], one container per entry of `containers`, each
/// writing the two lines `<marker>-first` and `<marker>-second` to its output.
///
/// `wait_for_start` says whether the caller waits for the cluster to report that the pod's
/// containers were started — which is what a case asserting that NOTHING was delivered has
/// to wait for, since a delivery that has not been attempted yet is not one that was
/// withheld. A case asserting that something WAS delivered waits for nothing here on
/// purpose: it makes no further Kubernetes API call at all, so what delivers its output is
/// the cluster's own clock rather than the next read.
fn pod_that_wrote(
    runtime: &tokio::runtime::Runtime,
    endpoint: &str,
    what: &str,
    pod: &str,
    containers: &[(&str, &str)],
    wait_for_start: bool,
) {
    let declared: Vec<Value> = containers
        .iter()
        .map(|(name, marker)| {
            json!({
                "name": name,
                "image": IMAGE,
                "command": ["sh", "-c", format!("echo {marker}-first; echo {marker}-second")],
            })
        })
        .collect();
    let created = k8s(
        runtime,
        endpoint,
        what,
        "POST",
        &format!("/api/v1/namespaces/{NAMESPACE}/pods"),
        Some(&json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": { "name": pod },
            "spec": { "restartPolicy": "Never", "containers": declared },
        })),
    );
    assert_eq!(
        created.status, 201,
        "{what}: the pod {pod:?} must be created, got {created:?}"
    );
    assert!(
        created.body["spec"]["nodeName"].is_string(),
        "{what}: the pod must be placed on the profile's Fargate capacity, got {created:?}"
    );
    if !wait_for_start {
        return;
    }
    // The containers' start is owed at a stored instant: the pod is read again until it
    // reports that it left the phase a pod nothing has started is in.
    for _ in 0..POLLS {
        let read = k8s(
            runtime,
            endpoint,
            what,
            "GET",
            &format!("/api/v1/namespaces/{NAMESPACE}/pods/{pod}"),
            None,
        );
        if read.body["status"]["phase"] != json!("Pending") {
            return;
        }
        runtime.block_on(async { tokio::time::sleep(POLL_INTERVAL).await });
    }
    panic!("{what}: the pod {pod:?} never had its containers started");
}

/// The group the router creates itself under `auto_create_group`.
const ROUTER_CREATED_GROUP: &str = "/fargate/router-created";
/// The group the ACCOUNT creates and the router is merely pointed at.
const ACCOUNT_HELD_GROUP: &str = "/fargate/account-held";
/// A group nobody creates, which the router was not asked to create either.
const ABSENT_GROUP: &str = "/fargate/absent";
/// A group an output names whose `Match` takes none of the pod's records.
const UNMATCHED_GROUP: &str = "/fargate/unmatched";
/// The one stream the `log_stream_name` output delivers every record to.
const FIXED_STREAM: &str = "app-stream";

/// The configuration of the granted cluster's router: four outputs, differing in how the
/// stream is named, who provisions the group, and whether the output takes the pod's
/// records.
fn granted_output_conf() -> String {
    format!(
        "# the router of the cluster under test\n\
         [OUTPUT]\n\
         \x20   Name cloudwatch_logs\n\
         \x20   Match kube.*\n\
         \x20   region {REGION}\n\
         \x20   log_group_name {ROUTER_CREATED_GROUP}\n\
         \x20   log_stream_prefix {STREAM_PREFIX}\n\
         \x20   auto_create_group true\n\
         \n\
         [OUTPUT]\n\
         \x20   Name cloudwatch_logs\n\
         \x20   Match kube.*\n\
         \x20   log_group_name {ACCOUNT_HELD_GROUP}\n\
         \x20   log_stream_name {FIXED_STREAM}\n\
         \n\
         [OUTPUT]\n\
         \x20   Name cloudwatch_logs\n\
         \x20   Match kube.*\n\
         \x20   log_group_name {ABSENT_GROUP}\n\
         \x20   log_stream_prefix {STREAM_PREFIX}\n\
         \n\
         [OUTPUT]\n\
         \x20   Name cloudwatch_logs\n\
         \x20   Match other.*\n\
         \x20   log_group_name {UNMATCHED_GROUP}\n\
         \x20   log_stream_prefix {STREAM_PREFIX}\n\
         \x20   auto_create_group true\n"
    )
}

/// The configuration of the withheld cluster's router: one output, into a group the account
/// holds, so the only thing that can stand between the pod's output and that group is the
/// Pod execution role's own grant.
fn withheld_output_conf(group: &str) -> String {
    format!(
        "[OUTPUT]\n\
         \x20   Name cloudwatch_logs\n\
         \x20   Match kube.*\n\
         \x20   region {REGION}\n\
         \x20   log_group_name {group}\n\
         \x20   log_stream_prefix {STREAM_PREFIX}\n\
         \x20   auto_create_group true\n"
    )
}

/// Create the log group `group` with the CALLER's own credential, which is how the account
/// provisions a destination it points a service at.
fn create_group(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime, group: &str) {
    let created = logs(
        router,
        runtime,
        "CreateLogGroup",
        &json!({ "logGroupName": group }),
    );
    assert_eq!(
        created.status, 200,
        "setup: the account's own log group {group:?} must be created, got {created:?}"
    );
}

/// The stream of `streams` that carries the records of the container `container` of `pod`:
/// the prefix the output configured, followed by the tag the router gives that container's
/// records — which names the pod, its namespace and the container.
fn stream_of_container<'held>(
    streams: &'held [String],
    pod: &str,
    container: &str,
) -> Option<&'held String> {
    streams.iter().find(|stream| {
        stream.starts_with(STREAM_PREFIX) && stream.contains(&format!("{pod}_{NAMESPACE}_{container}-"))
    })
}

#[test]
fn a_fargate_pods_output_is_delivered_as_the_profiles_pod_execution_role() {
    std::thread::Builder::new()
        .name("eks-fargate-pod-logs".to_string())
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

            let subnets = default_subnets(&router, &runtime);
            // Two Pod execution roles, both carrying the policy AWS publishes for one: the
            // first also carries the inline grant for the router's CloudWatch Logs calls,
            // the second carries nothing but the published policy — which is the role AWS's
            // own Fargate documentation prescribes and which grants no logs call.
            let granted_role = pod_execution_role(
                &router,
                &runtime,
                "eksFargateLoggingRole",
                Some(&ROUTER_ACTIONS),
            );
            let withheld_role =
                pod_execution_role(&router, &runtime, "eksFargateNoLoggingRole", None);
            let granted_endpoint = cluster_with_fargate_capacity(
                &router,
                &runtime,
                "fargate-logging-cluster",
                &granted_role,
                &subnets,
            );

            // The destinations the account provisions itself: the group one output is merely
            // pointed at. The other three are the router's to create or nobody's.
            create_group(&router, &runtime, ACCOUNT_HELD_GROUP);
            configure_router(
                &runtime,
                &granted_endpoint,
                "the granted cluster",
                &granted_output_conf(),
            );

            // ONE pod, TWO containers: each writes two lines, so each has a stream of its
            // own in the group and the lines of one are not the lines of the other.
            let pod = "logging-pod";
            let containers: [(&str, &str); 2] = [("app", "app-marker"), ("sidecar", "side-marker")];
            pod_that_wrote(
                &runtime,
                &granted_endpoint,
                "the granted cluster",
                pod,
                &containers,
                false,
            );

            // RULE, first half: the router created the group it was configured to create,
            // opened one stream per container under the configured prefix, and put every
            // line each container wrote into its own stream, in order.
            let created_streams = streams_until(&router, &runtime, ROUTER_CREATED_GROUP, 2);
            assert!(
                holds_group(&router, &runtime, ROUTER_CREATED_GROUP),
                "the router must create the group its auto_create_group output names, got none"
            );
            assert_eq!(
                created_streams.len(),
                containers.len(),
                "one stream per container that wrote, got {created_streams:?}"
            );
            for (container, marker) in containers {
                let case = format!("container {container:?}");
                let stream = stream_of_container(&created_streams, pod, container)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: expected a stream named from the configured prefix \
                             and the container's own tag, got {created_streams:?}"
                        )
                    });
                let held = events_until(&router, &runtime, ROUTER_CREATED_GROUP, stream, 2);
                assert_eq!(
                    held,
                    vec![format!("{marker}-first"), format!("{marker}-second")],
                    "case {case:?}: every line the container wrote is one event of its own \
                     stream, in order, got {held:?}"
                );
            }

            // The output naming a FIXED stream delivered into the group the ACCOUNT holds,
            // under that one name, and the records of both containers are in it.
            let held_streams = streams_until(&router, &runtime, ACCOUNT_HELD_GROUP, 1);
            assert_eq!(
                held_streams,
                vec![FIXED_STREAM.to_string()],
                "the fixed-stream output delivers into the one stream it names, got \
                 {held_streams:?}"
            );
            let held_events = events_until(
                &router,
                &runtime,
                ACCOUNT_HELD_GROUP,
                FIXED_STREAM,
                // Both containers' lines go to the one stream this output names.
                2 * containers.len(),
            );
            for (_, marker) in containers {
                assert!(
                    held_events.contains(&format!("{marker}-first")),
                    "every container's output reaches the group the account holds, got \
                     {held_events:?}"
                );
            }

            // A destination nobody created and the router was not asked to create takes the
            // records NOWHERE, and the account still holds no such group.
            assert!(
                !holds_group(&router, &runtime, ABSENT_GROUP),
                "an output pointed at a group nobody holds creates none, got one"
            );
            // An output whose Match takes none of the pod's records delivers nothing, so the
            // group it would have created is not created either.
            assert!(
                !holds_group(&router, &runtime, UNMATCHED_GROUP),
                "an output whose Match takes no record of this pod delivers nothing, got a \
                 group"
            );

            // RULE, second half: the SAME configuration under a Pod execution role no policy
            // grants the router's calls to delivers nothing — no stream and no event in the
            // group, which the account itself holds — and the same role, once granted,
            // delivers a later pod's output.
            let withheld_endpoint = cluster_with_fargate_capacity(
                &router,
                &runtime,
                "fargate-withheld-logging-cluster",
                &withheld_role,
                &subnets,
            );
            let withheld_group = "/fargate/withheld";
            create_group(&router, &runtime, withheld_group);
            configure_router(
                &runtime,
                &withheld_endpoint,
                "the withheld cluster",
                &withheld_output_conf(withheld_group),
            );
            pod_that_wrote(
                &runtime,
                &withheld_endpoint,
                "the withheld cluster",
                "withheld-pod",
                &[("app", "withheld-marker")],
                // Waited for: a delivery that has not been attempted yet is not one that
                // was withheld.
                true,
            );
            let withheld_streams = streams_of(&router, &runtime, withheld_group);
            assert!(
                withheld_streams.is_empty(),
                "a Pod execution role no policy grants the router's calls to opens no stream, \
                 got {withheld_streams:?}"
            );

            grant(&router, &runtime, "eksFargateNoLoggingRole", &ROUTER_ACTIONS);
            pod_that_wrote(
                &runtime,
                &withheld_endpoint,
                "the granted-after-the-fact cluster",
                "granted-later-pod",
                &[("app", "granted-later-marker")],
                false,
            );
            let granted_later = streams_until(&router, &runtime, withheld_group, 1);
            let stream = stream_of_container(&granted_later, "granted-later-pod", "app")
                .unwrap_or_else(|| {
                    panic!(
                        "once the role is granted the router's calls, the next pod's output is \
                         delivered, got {granted_later:?}"
                    )
                });
            let delivered = events_until(&router, &runtime, withheld_group, stream, 2);
            assert_eq!(
                delivered,
                vec![
                    "granted-later-marker-first".to_string(),
                    "granted-later-marker-second".to_string(),
                ],
                "the granted role's delivery carries what that pod wrote, got {delivered:?}"
            );
            // And the withheld pod's own output is still nowhere: the grant is not
            // retroactive, so the only stream in the group is the later pod's.
            assert_eq!(
                granted_later.len(),
                1,
                "the pod whose role was withheld delivered nothing, got {granted_later:?}"
            );

            // RULE, every call separately: a role granted the two CREATIONS but not the PUT
            // delivers nothing — each call the router makes is decided on its own, so one of
            // them being permitted is not the others being permitted. The whole delivery is
            // decided before any part of it is performed, so a delivery whose put is refused
            // leaves no group and no stream behind either.
            let partial_role = pod_execution_role(
                &router,
                &runtime,
                "eksFargatePartialLoggingRole",
                Some(&[
                    "logs:CreateLogGroup",
                    "logs:CreateLogStream",
                    "logs:DescribeLogStreams",
                ]),
            );
            let partial_endpoint = cluster_with_fargate_capacity(
                &router,
                &runtime,
                "fargate-partial-logging-cluster",
                &partial_role,
                &subnets,
            );
            let partial_group = "/fargate/partial";
            configure_router(
                &runtime,
                &partial_endpoint,
                "the partially granted cluster",
                &withheld_output_conf(partial_group),
            );
            pod_that_wrote(
                &runtime,
                &partial_endpoint,
                "the partially granted cluster",
                "partial-pod",
                &[("app", "partial-marker")],
                // Waited for: a put that has not been attempted yet is not one that was
                // withheld.
                true,
            );
            let partial_streams = streams_of(&router, &runtime, partial_group);
            assert!(
                partial_streams.is_empty(),
                "a role no policy grants the put to delivers nothing, so the stream the \
                 delivery would have opened is not opened either, got {partial_streams:?}"
            );
            assert!(
                !holds_group(&router, &runtime, partial_group),
                "and the group that delivery would have created is not created, got one"
            );
        })
        .expect("the suite thread spawns")
        .join()
        .expect("the suite thread completes");
}
