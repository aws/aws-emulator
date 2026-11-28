// SPDX-License-Identifier: CC0-1.0

//! Control plane logging turned on AFTER a stack deployed the cluster: a
//! CloudFormation-provisioned `AWS::EKS::Cluster` that declared no `Logging` gets the
//! same export any other cluster does once `UpdateClusterConfig` enables a log type.
//!
//! The RULE under test: **`LogSetup$enabled` is "If a log type is enabled, that log type
//! exports its control plane logs to CloudWatch Logs. If a log type isn't enabled, that
//! log type doesn't export its control plane logs", and that is a property of the
//! CLUSTER's stored configuration rather than of how the cluster came to exist — so a
//! cluster CloudFormation created with no `Logging` property has no
//! `/aws/eks/<cluster name>/cluster` group at all, and once an `UpdateClusterConfig`
//! enabling log types on it reports `Successful` that group holds one stream per enabled
//! type, named after the control plane component whose logs that type carries.**
//!
//! What this suite adds to the ones beside it — `cloudformation_eks_cluster_logging.rs`
//! (a stack whose template declares the types at CREATE time) and
//! `eks_disabled_logging_then_enabled_reaches_cloudwatch_logs.rs` (an update on a cluster
//! `CreateCluster` made) — is the combination of the two: the export of a cluster whose
//! create was made BY CLOUDFORMATION and whose logging was turned on afterwards, which is
//! the path a client that deploys its cluster from a template and then enables audit
//! logging takes. Three crates have to agree for it to hold (CloudFormation makes the
//! create, Amazon EKS provisions the export, CloudWatch Logs owns the group), so the
//! suite lives here.
//!
//! The rows differ along what the rule generalises over: how many types one enabling
//! update turns on (one, two, none at all for the control row) and WHICH ones they are —
//! including a type neither suite beside it enables through an update.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for.
const REGION: &str = "us-west-2";

/// How long one poll waits before reading again, and how many times: a stack's completion,
/// a cluster's activation and an update's completion are each owed from an instant no fixed
/// sleep can name, so the suite polls rather than sleeping once.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// See [`POLL_INTERVAL`]: 600 looks at 50ms is a 30-second deadline.
const POLLS: usize = 600;

/// The `StackStatus` a completed create reports.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `UpdateStatus` a completed update reports — the instant from which AWS creates the
/// cluster's log group ("`DescribeUpdate` ... the status of your cluster update").
const UPDATE_SUCCESSFUL: &str = "Successful";

/// The control plane component each log type's streams are named after, as the Amazon EKS
/// control plane logging documentation publishes them — one entry per member of `LogType`
/// (`smithy_query(query="shape", shape="LogType")`).
const COMPONENT_OF_LOG_TYPE: &[(&str, &str)] = &[
    ("api", "kube-apiserver"),
    ("audit", "kube-apiserver-audit"),
    ("authenticator", "authenticator"),
    ("controllerManager", "kube-controller-manager"),
    ("scheduler", "kube-scheduler"),
];

/// One answer, as this suite reads it.
#[derive(Debug)]
struct Answer {
    status: u16,
    text: String,
    body: Value,
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Drive one request at `signing_name`'s wire.
async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
    content_type: &str,
    target: Option<&str>,
    body: String,
) -> Answer {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer { status, text, body }
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&crate::query_wire::form_encode(key));
        body.push('=');
        body.push_str(&crate::query_wire::form_encode(value));
    }
    send(
        router,
        "cloudformation",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        None,
        body,
    )
    .await
}

/// Call one Amazon EKS operation over its restJson1 wire.
async fn eks(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    send(
        router,
        "eks",
        method,
        uri,
        "application/json",
        None,
        body.map(|json| json.to_string()).unwrap_or_default(),
    )
    .await
}

/// Call one Amazon CloudWatch Logs operation over its awsJson1_1 wire.
async fn logs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    send(
        router,
        "logs",
        "POST",
        "/",
        "application/x-amz-json-1.1",
        Some(&format!("Logs_20140328.{operation}")),
        body.to_string(),
    )
    .await
}

/// The text of the first `<name>…</name>` element of an XML body.
fn element(xml: &str, name: &str) -> Option<String> {
    crate::query_wire::elements(xml, name).into_iter().next()
}

/// Poll the stack until it reports a status its create has SETTLED in, and answer it.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let mut last = String::new();
    for _ in 0..POLLS {
        let answer = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: DescribeStacks must answer, got {answer:?}"
        );
        last = element(&answer.text, "StackStatus").unwrap_or_default();
        if !last.ends_with("_IN_PROGRESS") {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    last
}

/// The log group a cluster's control plane logs are exported to.
fn cluster_log_group(cluster: &str) -> String {
    format!("/aws/eks/{cluster}/cluster")
}

/// Whether CloudWatch Logs holds that group, found the way a client finds it:
/// `DescribeLogGroups` narrowed to the name as a prefix.
async fn holds_log_group(router: &MultiServiceRouter, case: &str, cluster: &str) -> bool {
    let group = cluster_log_group(cluster);
    let answer = logs(
        router,
        "DescribeLogGroups",
        &json!({ "logGroupNamePrefix": group }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: DescribeLogGroups must answer, got {answer:?}"
    );
    answer.body["logGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .any(|held| held["logGroupName"] == json!(group))
}

/// The streams of that group whose names start with `prefix`, read the way a client that
/// tails one component's logs reads them.
async fn streams_with_prefix(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    prefix: &str,
) -> Vec<String> {
    let answer = logs(
        router,
        "DescribeLogStreams",
        &json!({
            "logGroupName": cluster_log_group(cluster),
            "logStreamNamePrefix": prefix,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: DescribeLogStreams must answer, got {answer:?}"
    );
    answer.body["logStreams"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|held| held["logStreamName"].as_str().map(str::to_string))
        .collect()
}

/// The template one row deploys: the VPC and the two subnets the cluster is placed in,
/// both created by the stack itself, and the cluster — with NO `Logging` property at all,
/// which is the state every log type of a cluster starts in.
fn template(cluster: &str, cidr_prefix: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Net": {
                "Type": "AWS::EC2::VPC",
                "Properties": { "CidrBlock": format!("{cidr_prefix}.0.0/16") },
            },
            "SubnetA": {
                "Type": "AWS::EC2::Subnet",
                "Properties": {
                    "VpcId": { "Ref": "Net" },
                    "CidrBlock": format!("{cidr_prefix}.1.0/24"),
                    "AvailabilityZone": format!("{REGION}a"),
                },
            },
            "SubnetB": {
                "Type": "AWS::EC2::Subnet",
                "Properties": {
                    "VpcId": { "Ref": "Net" },
                    "CidrBlock": format!("{cidr_prefix}.2.0/24"),
                    "AvailabilityZone": format!("{REGION}b"),
                },
            },
            "Cluster": {
                "Type": "AWS::EKS::Cluster",
                "Properties": {
                    "Name": cluster,
                    "RoleArn": format!(
                        "arn:aws:iam::{}:role/eksClusterRole",
                        engine::emulator_account_id()
                    ),
                    "ResourcesVpcConfig": {
                        "SubnetIds": [{ "Ref": "SubnetA" }, { "Ref": "SubnetB" }],
                    },
                },
            },
        },
        "Outputs": { "ClusterName": { "Value": { "Ref": "Cluster" } } },
    })
    .to_string()
}

/// One row: the types the update enables on the stack's cluster.
struct Row {
    case: &'static str,
    stack: &'static str,
    cluster: &'static str,
    /// The second octet pair of the stack's own VPC, so each row's network is its own.
    cidr_prefix: &'static str,
    /// The types the update's `clusterLogging` enables, in the model's own order; EMPTY
    /// for the control row, whose update enables none.
    enabled: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "one type enabled after the stack deployed the cluster",
        stack: "eks-deployed-logging-api",
        cluster: "deployed-logging-api",
        cidr_prefix: "10.71",
        enabled: &["api"],
    },
    Row {
        case: "two types enabled at once, neither of them the first",
        stack: "eks-deployed-logging-pair",
        cluster: "deployed-logging-pair",
        cidr_prefix: "10.72",
        enabled: &["audit", "scheduler"],
    },
    Row {
        case: "a type no suite beside this one enables through an update",
        stack: "eks-deployed-logging-controller",
        cluster: "deployed-logging-controller",
        cidr_prefix: "10.73",
        enabled: &["controllerManager"],
    },
    Row {
        case: "an update that enables nothing, after which the group is still absent",
        stack: "eks-deployed-logging-none",
        cluster: "deployed-logging-none",
        cidr_prefix: "10.74",
        enabled: &[],
    },
];

/// The component a log type's stream is named after.
fn component_of(log_type: &str) -> &'static str {
    COMPONENT_OF_LOG_TYPE
        .iter()
        .find(|(declared, _)| *declared == log_type)
        .map(|(_, component)| *component)
        .unwrap_or_else(|| panic!("{log_type:?} is not a declared log type"))
}

#[tokio::test]
async fn enabling_logging_on_a_stack_deployed_cluster_creates_its_log_group() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);

    for row in ROWS {
        let case = row.case;
        let cluster = row.cluster;

        // THE STACK: a cluster CloudFormation creates, declaring no logging at all.
        let created = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template(cluster, row.cidr_prefix)),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateStack must be accepted, got {created:?}"
        );
        let status = settled_stack(&router, case, row.stack).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: a declaration CloudFormation can satisfy completes, got \
             {status:?} (events: {:?})",
            cloudformation(&router, "DescribeStackEvents", &[("StackName", row.stack)])
                .await
                .text
        );

        // While every log type is disabled — which is what a template declaring no
        // `Logging` leaves — the cluster has no control plane log group at all.
        assert!(
            !holds_log_group(&router, case, cluster).await,
            "case {case:?}: a cluster that enables no log type must have no {} group",
            cluster_log_group(cluster)
        );

        // THE UPDATE: the same change a client makes with `aws eks
        // update-cluster-config`, on the cluster the stack created.
        for _ in 0..POLLS {
            let described = eks(&router, "GET", &format!("/clusters/{cluster}"), None).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: the stack's cluster must be readable through Amazon EKS, \
                 got {described:?}"
            );
            if described.body["cluster"]["status"] == json!("ACTIVE") {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        let logging = if row.enabled.is_empty() {
            // The control row asks for the state the cluster is already in, stated the
            // way a client states it: every declared type, explicitly disabled.
            let every: Vec<&str> = COMPONENT_OF_LOG_TYPE
                .iter()
                .map(|(log_type, _)| *log_type)
                .collect();
            json!({ "clusterLogging": [{ "types": every, "enabled": false }] })
        } else {
            json!({ "clusterLogging": [{ "types": row.enabled, "enabled": true }] })
        };
        let updated = eks(
            &router,
            "POST",
            &format!("/clusters/{cluster}/update-config"),
            Some(json!({ "logging": logging })),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateClusterConfig must answer 200, got {updated:?}"
        );
        let update_id = updated.body["update"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the update must report an id, got {updated:?}"))
            .to_string();

        // AWS creates the group once the update reports `Successful`, so that is the
        // instant this row waits for — polled, because nothing sleeps for it.
        let mut update_status = String::new();
        for _ in 0..POLLS {
            let described = eks(
                &router,
                "GET",
                &format!("/clusters/{cluster}/updates/{update_id}"),
                None,
            )
            .await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: DescribeUpdate must answer, got {described:?}"
            );
            update_status = described.body["update"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if update_status == UPDATE_SUCCESSFUL {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        assert_eq!(
            update_status, UPDATE_SUCCESSFUL,
            "case {case:?}: the logging update must complete, got {update_status:?}"
        );

        // WHAT THE EXPORT CREATED: the cluster's own group, with one stream per enabled
        // type — and nothing at all for the update that enabled none.
        let held = holds_log_group(&router, case, cluster).await;
        assert_eq!(
            held,
            !row.enabled.is_empty(),
            "case {case:?}: the group {} must exist exactly when the update enabled a \
             type, got held={held}",
            cluster_log_group(cluster)
        );
        for log_type in row.enabled {
            let component = component_of(log_type);
            // A client tails one component by its stream-name PREFIX, which is how the
            // group is read for `api`'s `kube-apiserver-` streams.
            let prefix = format!("{component}-");
            let found = streams_with_prefix(&router, case, cluster, &prefix).await;
            assert_eq!(
                found.len(),
                1,
                "case {case:?}: {log_type:?} must export under exactly one {prefix:?} \
                 stream, got {found:?}"
            );
        }
    }
}
