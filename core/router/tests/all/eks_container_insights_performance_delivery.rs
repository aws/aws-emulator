// SPDX-License-Identifier: CC0-1.0

//! The CONTAINER INSIGHTS performance records a cluster running the CloudWatch agent
//! add-on delivers, and the IAM grant that delivery is held to.
//!
//! The RULE under test: **while the `amazon-cloudwatch-observability` add-on is `ACTIVE`
//! on a cluster, the CloudWatch agent it runs on that cluster's nodes delivers the
//! cluster's performance to CloudWatch Logs — into
//! `/aws/containerinsights/<cluster name>/performance`, as Embedded Metric Format
//! documents carrying a `CloudWatchMetrics` declaration and the cluster's own name — and
//! it delivers that AS THE IAM ROLE the account put in force on its Kubernetes service
//! account (the EKS Pod Identity association for it, and otherwise the role of the node
//! it runs on), so an agent whose role is not permitted the CloudWatch Logs writes a
//! delivery makes delivers nothing, and starts delivering as soon as that grant is
//! attached to the same role.** A cluster running no such add-on runs no agent, so it has
//! no such group at all.
//!
//! Four crates have to agree for it to hold — Amazon EKS installs the add-on and runs the
//! agent, IAM holds the trust policy and the documents in force on the role, Amazon
//! CloudWatch Logs owns the group the records land in, and Amazon EC2 owns the subnets the
//! cluster and its node group sit in — so the suite lives here rather than in any one of
//! them.
//!
//! The rows differ along what the rule generalises over: WHICH role is in force on the
//! agent (one bound through an EKS Pod Identity association, and the node's own role for a
//! cluster that binds none) and whether that role is permitted the writes (granted,
//! withheld, and withheld-then-granted, which is the contrast that makes a missing
//! permission observably different from an agent that produces nothing). The control row
//! installs no add-on at all.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread. The control-plane drivers are the test crate's own
//! (`eks_wire`); the CloudWatch Logs half is one more driver here, because that wire
//! carries an `X-Amz-Target` no other suite's driver sends.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::eks_wire::{cluster_body, ec2, eks, iam, role_arn, Answer, REGION};
use crate::query_wire::elements;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// How often a clock-owed read is looked at again, and how many times in all: nothing
/// inside the emulator sleeps for a due instant, so a condition is polled rather than
/// waited for, with a deadline far above every span this service owes.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
/// See [`POLL_INTERVAL`]: 600 looks at 50ms is a 30-second deadline.
const POLLS: usize = 600;

/// How many times a row that expects NO delivery looks for one before it concludes there
/// is none.
///
/// It is the same number of looks the rows that DO expect a delivery are answered within
/// many times over, so a row asserting absence has given delivery every chance the others
/// needed; it is a count of looks rather than a span of the clock.
const ABSENCE_LOOKS: usize = 120;

/// The add-on that runs the CloudWatch agent, by the name Amazon EKS publishes it under.
const OBSERVABILITY_ADDON: &str = "amazon-cloudwatch-observability";

/// The Kubernetes namespace that add-on installs into and the service account its agent
/// runs as — the pair an EKS Pod Identity association binds a role to. Both are read off
/// `DescribeAddonConfiguration`'s published pod-identity configuration in the assertions
/// below rather than assumed here.
const AGENT_NAMESPACE: &str = "amazon-cloudwatch";
const AGENT_SERVICE_ACCOUNT: &str = "cloudwatch-agent";

/// The AWS-managed policies the rows attach. The agent's is the one Amazon Web Services
/// publishes for a CloudWatch agent (it carries `logs:CreateLogGroup`,
/// `logs:CreateLogStream` and `logs:PutLogEvents`); the node policies are the ones the
/// Amazon EKS node IAM role carries, neither of which grants a CloudWatch Logs write.
const AGENT_POLICY: &str = "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy";
const WORKER_NODE_POLICY: &str = "arn:aws:iam::aws:policy/AmazonEKSWorkerNodePolicy";
const CNI_POLICY: &str = "arn:aws:iam::aws:policy/AmazonEKS_CNI_Policy";

/// The member a performance document declares its metrics under, and the member it names
/// its cluster under — what makes a delivered record an EMF document about that cluster.
const METRIC_DIRECTIVE_MEMBER: &str = "CloudWatchMetrics";
const CLUSTER_NAME_MEMBER: &str = "ClusterName";

/// The log group one cluster's performance records are delivered to.
fn performance_group(cluster: &str) -> String {
    format!("/aws/containerinsights/{cluster}/performance")
}

/// Drive one Amazon CloudWatch Logs operation over its awsJson1_1 wire.
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
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
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

/// Create the IAM role `name`, trusted by `principal` as that kind of role is, carrying
/// the AWS-managed policies `policies` — and answer its ARN.
fn created_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    principal: &str,
    policies: &[&str],
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": ["sts:AssumeRole", "sts:TagSession"],
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
        "setup: the role {name:?} must be created, got {created:?}"
    );
    for policy in policies {
        attach_policy(router, runtime, name, policy);
    }
    role_arn(name)
}

/// Attach one AWS-managed policy to a role that already exists.
fn attach_policy(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_name: &str,
    policy: &str,
) {
    let attached = iam(
        router,
        runtime,
        "AttachRolePolicy",
        &[("RoleName", role_name), ("PolicyArn", policy)],
    );
    assert_eq!(
        attached.status, 200,
        "setup: {policy} must attach to {role_name:?}, got {attached:?}"
    );
}

/// Create the cluster `name` with one node group running under `node_role`, and wait for
/// both to be ACTIVE — the capacity the agent's own pods run on.
fn cluster_with_a_node(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    node_role: &str,
    subnets: &[String],
) {
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
    read_until(
        "the cluster to reach ACTIVE",
        || eks(router, runtime, "GET", &format!("/clusters/{name}"), None),
        |answer| answer.body["cluster"]["status"] == json!("ACTIVE"),
    );
    let nodegroup = eks(
        router,
        runtime,
        "POST",
        &format!("/clusters/{name}/node-groups"),
        Some(json!({
            "nodegroupName": "agents",
            "nodeRole": node_role,
            "subnets": subnets,
            "scalingConfig": { "minSize": 1, "maxSize": 1, "desiredSize": 1 },
        })),
    );
    assert_eq!(
        nodegroup.status, 200,
        "setup: the node group of {name:?} must be created, got {nodegroup:?} ({})",
        nodegroup.message()
    );
    read_until(
        "the node group to reach ACTIVE",
        || {
            eks(
                router,
                runtime,
                "GET",
                &format!("/clusters/{name}/node-groups/agents"),
                None,
            )
        },
        |answer| answer.body["nodegroup"]["status"] == json!("ACTIVE"),
    );
}

/// Bind `agent_role` to the agent's `(namespace, service account)` pair through an EKS Pod
/// Identity association, the way a client turns Container Insights on.
fn bind_agent_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cluster: &str,
    agent_role: &str,
) {
    let created = eks(
        router,
        runtime,
        "POST",
        &format!("/clusters/{cluster}/pod-identity-associations"),
        Some(json!({
            "namespace": AGENT_NAMESPACE,
            "serviceAccount": AGENT_SERVICE_ACCOUNT,
            "roleArn": agent_role,
        })),
    );
    assert_eq!(
        created.status, 200,
        "setup: the pod identity association of {cluster:?} must be created, got {created:?} ({})",
        created.message()
    );
}

/// Install the CloudWatch agent add-on on `cluster` and wait for it to report ACTIVE.
fn install_observability_addon(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cluster: &str,
) {
    let created = eks(
        router,
        runtime,
        "POST",
        &format!("/clusters/{cluster}/addons"),
        Some(json!({ "addonName": OBSERVABILITY_ADDON })),
    );
    assert_eq!(
        created.status, 200,
        "setup: the add-on must be installed on {cluster:?}, got {created:?} ({})",
        created.message()
    );
    read_until(
        "the observability add-on to reach ACTIVE",
        || describe_addon(router, runtime, cluster),
        |answer| answer.body["addon"]["status"] == json!("ACTIVE"),
    );
}

/// Read the add-on back — the call a client polls while it waits for the agent, and the
/// one that materialises what the agent's collection interval has made due.
fn describe_addon(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cluster: &str,
) -> Answer {
    eks(
        router,
        runtime,
        "GET",
        &format!("/clusters/{cluster}/addons/{OBSERVABILITY_ADDON}"),
        None,
    )
}

/// The messages the cluster's performance log group holds, as a consumer of the group
/// reads them: `FilterLogEvents` over the whole group.
///
/// EMPTY for a group CloudWatch Logs does not hold at all, which is what a cluster that
/// has delivered nothing leaves.
fn performance_messages(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cluster: &str,
) -> Vec<String> {
    let answer = logs(
        router,
        runtime,
        "FilterLogEvents",
        &json!({ "logGroupName": performance_group(cluster) }),
    );
    if answer.status != 200 {
        return Vec::new();
    }
    answer.body["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|event| event["message"].as_str().map(str::to_string))
        .collect()
}

/// Look for a delivery into `cluster`'s performance group, polling the add-on between
/// looks so the collections its interval has made due are materialised, and answer the
/// messages found — empty when `looks` looks found none.
fn delivered_messages(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cluster: &str,
    looks: usize,
) -> Vec<String> {
    for _ in 0..looks {
        let found = performance_messages(router, runtime, cluster);
        if !found.is_empty() {
            return found;
        }
        // The add-on is read the way a client waiting on the agent reads it, which is
        // also what materialises a collection the clock has made due.
        describe_addon(router, runtime, cluster);
        std::thread::sleep(POLL_INTERVAL);
    }
    Vec::new()
}

/// Assert that every message of a delivery is an EMF document about `cluster`.
fn assert_emf_about(case: &str, cluster: &str, messages: &[String]) {
    assert!(
        !messages.is_empty(),
        "case {case:?}: the performance group of {cluster:?} must receive events, got none"
    );
    for message in messages {
        let document: Value = serde_json::from_str(message).unwrap_or_else(|error| {
            panic!("case {case:?}: every record must parse as JSON, got {message:?} ({error})")
        });
        assert!(
            document.get(METRIC_DIRECTIVE_MEMBER).is_some(),
            "case {case:?}: every record must declare its metrics under \
             {METRIC_DIRECTIVE_MEMBER}, got {document}"
        );
        assert_eq!(
            document[CLUSTER_NAME_MEMBER],
            json!(cluster),
            "case {case:?}: every record must name its cluster, got {document}"
        );
        let metrics = document[METRIC_DIRECTIVE_MEMBER][0]["Metrics"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !metrics.is_empty(),
            "case {case:?}: a declaration must name the figures it covers, got {document}"
        );
        for metric in &metrics {
            let name = metric["Name"].as_str().unwrap_or_default();
            assert!(
                document.get(name).is_some_and(Value::is_number),
                "case {case:?}: the declared figure {name:?} must be carried as a number, \
                 got {document}"
            );
        }
    }
}

/// Which role a row puts in force on the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentRole {
    /// A role bound to the agent's service account, permitted the writes.
    BoundAndGranted,
    /// A role bound to the agent's service account that no policy permits them to — and
    /// which the row then attaches the grant to.
    BoundAndWithheld,
    /// No binding at all, so the agent acts as the role of the node it runs on, which
    /// carries the grant here.
    TheNodesOwnRole,
    /// No add-on, so no agent: the control row.
    NoAgent,
}

/// One row: the cluster, which role is in force on its agent and whether a delivery must
/// be observed.
struct Row {
    case: &'static str,
    cluster: &'static str,
    node_role: &'static str,
    node_policies: &'static [&'static str],
    agent: AgentRole,
}

const ROWS: &[Row] = &[
    Row {
        case: "an agent whose bound role carries the CloudWatch agent policy",
        cluster: "insights-granted",
        node_role: "insightsGrantedNodeRole",
        node_policies: &[WORKER_NODE_POLICY, CNI_POLICY],
        agent: AgentRole::BoundAndGranted,
    },
    Row {
        case: "an agent whose bound role carries no CloudWatch write, then is given one",
        cluster: "insights-withheld",
        node_role: "insightsWithheldNodeRole",
        node_policies: &[WORKER_NODE_POLICY],
        agent: AgentRole::BoundAndWithheld,
    },
    Row {
        case: "an agent with no bound role, acting as the node's own role",
        cluster: "insights-node-role",
        node_role: "insightsAgentNodeRole",
        node_policies: &[WORKER_NODE_POLICY, AGENT_POLICY],
        agent: AgentRole::TheNodesOwnRole,
    },
    Row {
        case: "a cluster running no observability add-on",
        cluster: "insights-no-addon",
        node_role: "insightsBareNodeRole",
        node_policies: &[WORKER_NODE_POLICY, CNI_POLICY],
        agent: AgentRole::NoAgent,
    },
];

/// The service account the add-on's published pod-identity configuration names, read out
/// of `DescribeAddonConfiguration` rather than written here — so the pair the rows bind a
/// role to is the pair Amazon EKS publishes for this add-on.
fn published_agent_service_account(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cluster: &str,
) -> String {
    let version = describe_addon(router, runtime, cluster).body["addon"]["addonVersion"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_default();
    let answer = eks(
        router,
        runtime,
        "GET",
        &format!(
            "/addons/configuration-schemas?addonName={OBSERVABILITY_ADDON}\
             &addonVersion={version}"
        ),
        None,
    );
    assert_eq!(
        answer.status, 200,
        "setup: the add-on's configuration must be published, got {answer:?}"
    );
    answer.body["podIdentityConfiguration"][0]["serviceAccount"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_default()
}

#[test]
fn a_cluster_running_the_cloudwatch_agent_addon_delivers_its_performance_records() {
    std::thread::Builder::new()
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
            let withheld_agent_role = created_role(
                &router,
                &runtime,
                "insightsWithheldAgentRole",
                "pods.eks.amazonaws.com",
                &[],
            );
            let granted_agent_role = created_role(
                &router,
                &runtime,
                "insightsGrantedAgentRole",
                "pods.eks.amazonaws.com",
                &[AGENT_POLICY],
            );

            for row in ROWS {
                let case = row.case;
                let cluster = row.cluster;
                let node_role = created_role(
                    &router,
                    &runtime,
                    row.node_role,
                    "ec2.amazonaws.com",
                    row.node_policies,
                );
                cluster_with_a_node(&router, &runtime, cluster, &node_role, &subnets);
                match row.agent {
                    AgentRole::BoundAndGranted => {
                        bind_agent_role(&router, &runtime, cluster, &granted_agent_role);
                    }
                    AgentRole::BoundAndWithheld => {
                        bind_agent_role(&router, &runtime, cluster, &withheld_agent_role);
                    }
                    AgentRole::TheNodesOwnRole | AgentRole::NoAgent => {}
                }
                if row.agent == AgentRole::NoAgent {
                    // THE CONTROL: no add-on, so no agent, so no group at all — which is
                    // what makes an installed add-on observably different.
                    let found =
                        performance_messages(&router, &runtime, cluster);
                    assert!(
                        found.is_empty(),
                        "case {case:?}: a cluster running no agent must have no performance \
                         records, got {found:?}"
                    );
                    continue;
                }
                install_observability_addon(&router, &runtime, cluster);
                // The pair the rows bind a role to is the one the add-on publishes.
                let published = published_agent_service_account(&router, &runtime, cluster);
                assert_eq!(
                    published, AGENT_SERVICE_ACCOUNT,
                    "case {case:?}: the add-on's published service account must be the one \
                     this suite binds, got {published:?}"
                );

                if row.agent == AgentRole::BoundAndWithheld {
                    // No grant, no delivery: the agent is running and the group stays
                    // empty, which is the half a client cannot tell from "the add-on
                    // produces nothing" unless the other half works.
                    let found =
                        delivered_messages(&router, &runtime, cluster, ABSENCE_LOOKS);
                    assert!(
                        found.is_empty(),
                        "case {case:?}: an agent whose role is not permitted the write must \
                         deliver nothing, got {found:?}"
                    );
                    // And the SAME role, given the grant, starts delivering — nothing is
                    // recreated.
                    attach_policy(
                        &router,
                        &runtime,
                        "insightsWithheldAgentRole",
                        AGENT_POLICY,
                    );
                }

                let delivered = delivered_messages(&router, &runtime, cluster, POLLS);
                assert_emf_about(case, cluster, &delivered);
                // One stream per node of the cluster, NAMED after the node: the nodes the
                // delivered node-level documents name are exactly the streams the group
                // holds, so a stream named after anything else fails here.
                let streams = logs(
                    &router,
                    &runtime,
                    "DescribeLogStreams",
                    &json!({ "logGroupName": performance_group(cluster) }),
                );
                assert_eq!(
                    streams.status, 200,
                    "case {case:?}: DescribeLogStreams must answer, got {streams:?}"
                );
                let mut stream_names: Vec<String> = streams.body["logStreams"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|held| held["logStreamName"].as_str().map(str::to_string))
                    .collect();
                stream_names.sort();
                let documents: Vec<Value> = delivered
                    .iter()
                    .filter_map(|message| serde_json::from_str::<Value>(message).ok())
                    .collect();
                let mut nodes_named: Vec<String> = documents
                    .iter()
                    .filter(|document| document["Type"] == json!("Node"))
                    .filter_map(|document| document["NodeName"].as_str().map(str::to_string))
                    .collect();
                nodes_named.sort();
                nodes_named.dedup();
                assert!(
                    !nodes_named.is_empty(),
                    "case {case:?}: a node-level document must name the node it was \
                     collected for, got {documents:?}"
                );
                assert_eq!(
                    stream_names, nodes_named,
                    "case {case:?}: the group's streams must be the cluster's nodes, got \
                     {stream_names:?} against {nodes_named:?}"
                );
                // Both document kinds are delivered: the cluster's own figures and the
                // node's, which is how a reader of the group tells them apart.
                let kinds: Vec<String> = documents
                    .iter()
                    .filter_map(|document| document["Type"].as_str().map(str::to_string))
                    .collect();
                for kind in ["Cluster", "Node"] {
                    assert!(
                        kinds.iter().any(|held| held == kind),
                        "case {case:?}: the delivery must carry a {kind:?} document, got \
                         {kinds:?}"
                    );
                }
            }
        })
        .expect("a sized thread")
        .join()
        .expect("the suite to run");
}
