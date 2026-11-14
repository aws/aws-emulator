// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack's declared Amazon ECS cluster EXISTS in Amazon ECS — and the
//! stack's outputs name it.
//!
//! The rule under test: **a stack whose template declares an `AWS::ECS::Cluster` creates
//! that cluster in Amazon ECS, so the first ECS read a client makes against the cluster
//! the stack's outputs name finds it** — `ListTasks` answers with the cluster's tasks
//! (possibly none) rather than with `ClusterNotFoundException`, which is a not-found that
//! points at the caller rather than at the stack. Two crates have to agree for that to
//! hold (CloudFormation makes the call, Amazon ECS serves it), so the suite lives here:
//! a crate-local test can only ever see one side of the call.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! cluster or leaves the name to CloudFormation, which cluster properties it declares
//! (settings, tags, neither), and whether other container-platform resources are declared
//! beside it — a registry, and the network, target group and load balancer an ECS service
//! runs behind, every one of which this emulator creates in the service that owns it and
//! whose documented identifiers the stack reports.
//!
//! Deleting the stack deletes the cluster again: the last case drives that, because a
//! resource a stack created and no longer records must not be left behind in the service
//! that holds it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and so the region the stack's cluster and
/// its ARN belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due.
const PAST_THE_SPAN: Duration = Duration::from_millis(900);

/// How long between two readings of a stack that is still being created, and how many
/// readings before a wait gives up: a generous deadline, because what is waited for is the
/// owning services' own creation spans rather than anything this suite times.
const LOOK_AGAIN: Duration = Duration::from_millis(25);
const LOOKS: usize = 1_200;

/// The `StackStatus` a create reports while it is still running, which is the CONDITION
/// [`stack_once_settled`] waits out.
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// The emulator with the services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every
/// other suite here; the in-process callback CloudFormation makes signs its request
/// either way (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon ECR, for the `AWS::ECR::Repository` one row declares beside the cluster: the
    // stack really creates it there, so the service that owns it is mounted beside the
    // rest or that resource's create is refused and the stack fails.
    router.register_service(
        service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon EC2 and both generations of Elastic Load Balancing, for the VPC, the subnets,
    // the target group and the load balancer one row declares beside the cluster: the stack
    // really creates each of them there, and the CLASSIC service is the front door that
    // routes an ELBv2 envelope to the ELBv2 model (the two share one endpoint and one
    // signing name), so both are mounted exactly as `core/router/src/lib.rs` mounts them.
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` value the way an AWS SDK does,
/// so a template body full of `{`, `"` and `&` survives the body.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Amazon ECS operation over its awsJson1_1 wire.
async fn ecs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ecs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerServiceV20141113.{operation}"),
        )
        .header("authorization", authorization("ecs"))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Every `<tag>` text of an XML body, in document order, with the serializer's escaping
/// undone.
fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(
            after[..end]
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&"),
        );
        rest = &after[end + close.len()..];
    }
    found
}

fn xml_text(xml: &str, tag: &str) -> Option<String> {
    xml_texts(xml, tag).into_iter().next()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as — the
/// very read the issue filed: each `member` carries exactly one `OutputKey` and one
/// `OutputValue`, so an output reported with no value shifts the pairing and is visible
/// here as a missing or mismatched key.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    xml_texts(xml, "OutputKey")
        .into_iter()
        .zip(xml_texts(xml, "OutputValue"))
        .collect()
}

/// Create a stack from `template` and answer its id.
async fn create_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    xml_text(&body, "StackId").unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {body}"))
}

/// The one stack `stack_name` names, as the response body.
async fn describe_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// The same stack once its create has SETTLED: `DescribeStacks` read again until the status is
/// no longer [`CREATE_IN_PROGRESS`], or until the deadline passes.
///
/// Waits for the CONDITION rather than for a span of the clock — what the create waits on is
/// the owning services' own spans, which is not something this suite can time — and answers
/// the LAST body read either way, so the assertions on it report what CloudFormation really
/// said rather than how fast the host ran.
async fn stack_once_settled(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let mut last = describe_stack(router, case, stack_name).await;
    for _ in 0..LOOKS {
        if xml_text(&last, "StackStatus").as_deref() != Some(CREATE_IN_PROGRESS) {
            return last;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
        last = describe_stack(router, case, stack_name).await;
    }
    last
}

/// One row of the table: a template, and the output key carrying the cluster's name.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// The template body, in the form the row writes it.
    template: &'static str,
    /// The cluster name the stack must report, or `None` where CloudFormation generates it
    /// (the row then only asserts the FORM, `<stack>-<logical id>-<unique id>`).
    cluster_name: Option<&'static str>,
    /// Every other output the stack must report WITH a value, whatever that value is: the
    /// identifiers of the resources declared beside the cluster.
    other_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a cluster named by the template, with settings and tags",
        stack_name: "shop-platform",
        template: concat!(
            "Resources:\n",
            "  AppCluster:\n",
            "    Type: AWS::ECS::Cluster\n",
            "    Properties:\n",
            "      ClusterName: shop-cluster\n",
            "      ClusterSettings:\n",
            "        - Name: containerInsights\n",
            "          Value: enabled\n",
            "      Tags:\n",
            "        - Key: app\n",
            "          Value: shop\n",
            "Outputs:\n",
            "  ClusterName:\n",
            "    Value: !Ref AppCluster\n",
            "  ClusterArn:\n",
            "    Value: !GetAtt AppCluster.Arn\n",
        ),
        cluster_name: Some("shop-cluster"),
        other_outputs: &["ClusterArn"],
    },
    Row {
        case: "a cluster the template does not name, beside the rest of an app stack",
        stack_name: "warehouse",
        template: concat!(
            "Resources:\n",
            "  Cluster:\n",
            "    Type: AWS::ECS::Cluster\n",
            "  Registry:\n",
            "    Type: AWS::ECR::Repository\n",
            "    Properties:\n",
            "      RepositoryName: warehouse-images\n",
            "  Vpc:\n",
            "    Type: AWS::EC2::VPC\n",
            "    Properties:\n",
            "      CidrBlock: 10.7.0.0/16\n",
            "  SubnetA:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref Vpc\n",
            "      CidrBlock: 10.7.1.0/24\n",
            "      AvailabilityZone: us-east-1a\n",
            "  SubnetB:\n",
            "    Type: AWS::EC2::Subnet\n",
            "    Properties:\n",
            "      VpcId: !Ref Vpc\n",
            "      CidrBlock: 10.7.2.0/24\n",
            "      AvailabilityZone: us-east-1b\n",
            "  TargetGroup:\n",
            "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
            "    Properties:\n",
            "      Name: warehouse-tg\n",
            "      Port: 80\n",
            "      Protocol: HTTP\n",
            "      TargetType: ip\n",
            "      VpcId: !Ref Vpc\n",
            "  LoadBalancer:\n",
            "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
            "    Properties:\n",
            "      Name: warehouse-alb\n",
            "      Subnets:\n",
            "        - !Ref SubnetA\n",
            "        - !Ref SubnetB\n",
            "Outputs:\n",
            "  ClusterName:\n",
            "    Value: !Ref Cluster\n",
            "  RepositoryUri:\n",
            "    Value: !GetAtt Registry.RepositoryUri\n",
            "  TargetGroupArn:\n",
            "    Value: !Ref TargetGroup\n",
            "  LoadBalancerArn:\n",
            "    Value: !Ref LoadBalancer\n",
            "  LoadBalancerDnsName:\n",
            "    Value: !GetAtt LoadBalancer.DNSName\n",
        ),
        cluster_name: None,
        other_outputs: &[
            "RepositoryUri",
            "TargetGroupArn",
            "LoadBalancerArn",
            "LoadBalancerDnsName",
        ],
    },
    Row {
        case: "a cluster declared in the JSON spelling, with no properties at all",
        stack_name: "platform-min",
        template: r#"{"Resources":{"Cluster":{"Type":"AWS::ECS::Cluster",
             "Properties":{"ClusterName":"minimal-cluster"}}},
           "Outputs":{"ClusterName":{"Value":{"Ref":"Cluster"}}}}"#,
        cluster_name: Some("minimal-cluster"),
        other_outputs: &[],
    },
];

#[tokio::test]
async fn a_stacks_declared_ecs_cluster_takes_reads_in_ecs() {
    let router = build_router();
    for row in ROWS {
        create_stack(&router, row.case, row.stack_name, row.template).await;
        tokio::time::sleep(PAST_THE_SPAN).await;
        let described = describe_stack(&router, row.case, row.stack_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {:?}: expected the stack to have completed; status reason {:?}",
            row.case,
            xml_text(&described, "StackStatusReason")
        );

        // Every declared output carries a value, which is what the standard
        // `{o["OutputKey"]: o["OutputValue"]}` read needs.
        let outputs = outputs_of(&described);
        let cluster = outputs.get("ClusterName").unwrap_or_else(|| {
            panic!(
                "case {:?}: no ClusterName output with a value, got {outputs:?}",
                row.case
            )
        });
        match row.cluster_name {
            Some(expected) => assert_eq!(
                cluster, expected,
                "case {:?}: expected the cluster the template named",
                row.case
            ),
            // "CloudFormation generates a unique physical ID and uses that ID for the
            // resource name", in the form `<stack name>-<logical id>-<unique id>`.
            None => assert!(
                cluster.starts_with(&format!("{}-Cluster-", row.stack_name)),
                "case {:?}: expected CloudFormation's generated cluster name, got {cluster:?}",
                row.case
            ),
        }
        for key in row.other_outputs {
            let value = outputs.get(*key).unwrap_or_else(|| {
                panic!(
                    "case {:?}: no {key:?} output with a value, got {outputs:?}",
                    row.case
                )
            });
            assert!(
                !value.is_empty() && !value.contains("${"),
                "case {:?}: output {key:?} carries {value:?}",
                row.case
            );
        }

        // The read the issue filed: it answers `ClusterNotFoundException` for a cluster
        // CloudFormation reported as created but never created.
        let (status, listed) = ecs(&router, "ListTasks", &json!({ "cluster": cluster })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ecs ListTasks on the stack's cluster {cluster:?} to \
             answer 200, got {status} with body {listed}",
            row.case
        );

        // …and Amazon ECS holds exactly the cluster the stack reports, under the ARN the
        // stack's other output names.
        let (status, described_cluster) =
            ecs(&router, "DescribeClusters", &json!({ "clusters": [cluster] })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ecs DescribeClusters to answer 200, got {status} with \
             body {described_cluster}",
            row.case
        );
        assert_eq!(
            described_cluster["clusters"][0]["clusterName"].as_str(),
            Some(cluster.as_str()),
            "case {:?}: expected Amazon ECS to hold cluster {cluster:?}, got \
             {described_cluster}",
            row.case
        );
        assert_eq!(
            described_cluster["clusters"][0]["clusterArn"].as_str(),
            Some(format!("arn:aws:ecs:{REGION}:{ACCOUNT_ID}:cluster/{cluster}").as_str()),
            "case {:?}: expected the cluster's documented ARN, got {described_cluster}",
            row.case
        );
        if let Some(reported_arn) = outputs.get("ClusterArn") {
            assert_eq!(
                described_cluster["clusters"][0]["clusterArn"].as_str(),
                Some(reported_arn.as_str()),
                "case {:?}: expected the stack's ClusterArn output to be the ARN Amazon \
                 ECS holds",
                row.case
            );
        }
        assert_eq!(
            described_cluster["failures"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0),
            0,
            "case {:?}: expected no failures describing {cluster:?}, got \
             {described_cluster}",
            row.case
        );
    }
}

/// Deleting the stack deletes the cluster it created: a resource the stack no longer
/// records must not be left behind in the service that holds it.
#[tokio::test]
async fn deleting_the_stack_deletes_the_cluster_it_created() {
    let router = build_router();
    let case = "a stack whose one resource is a named cluster";
    let template = concat!(
        "Resources:\n",
        "  Cluster:\n",
        "    Type: AWS::ECS::Cluster\n",
        "    Properties:\n",
        "      ClusterName: transient-cluster\n",
        "Outputs:\n",
        "  ClusterName:\n",
        "    Value: !Ref Cluster\n",
    );
    create_stack(&router, case, "transient-stack", template).await;
    tokio::time::sleep(PAST_THE_SPAN).await;
    let (status, _) = ecs(
        &router,
        "ListTasks",
        &json!({ "cluster": "transient-cluster" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the cluster to exist before the delete"
    );

    let (status, body) = cloudformation(
        &router,
        "DeleteStack",
        &[("StackName", "transient-stack")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_SPAN).await;

    let (status, listed) = ecs(
        &router,
        "ListTasks",
        &json!({ "cluster": "transient-cluster" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the cluster to be gone with the stack, got {status} with \
         body {listed}"
    );
    assert!(
        listed["__type"]
            .as_str()
            .unwrap_or_default()
            .ends_with("ClusterNotFoundException"),
        "case {case:?}: expected ClusterNotFoundException once the stack deleted its \
         cluster, got {listed}"
    );
}

/// An `AWS::ECS::Service` declared on a cluster the SAME template declares reports the ARN
/// Amazon ECS identifies it by, and that ARN carries the cluster the stack really created.
///
/// A service ARN is `service/<cluster>/<name>`, so the value a stack hands back is only
/// addressable if the cluster segment names the cluster the stack created — which the
/// declaration names by `Ref` of another resource, the case a crate-local suite cannot
/// drive because the cluster has to EXIST in Amazon ECS for the stack to complete.
#[tokio::test]
async fn an_ecs_services_reported_arn_carries_the_cluster_the_stack_created() {
    let router = build_router();
    let case = "a service on a cluster the same template declares";
    let template = concat!(
        "Resources:\n",
        "  AppCluster:\n",
        "    Type: AWS::ECS::Cluster\n",
        "    Properties:\n",
        "      ClusterName: checkout-cluster\n",
        "  AppTask:\n",
        "    Type: AWS::ECS::TaskDefinition\n",
        "    Properties:\n",
        "      Family: checkout-task\n",
        "      ContainerDefinitions:\n",
        "        - Name: app\n",
        "          Image: public.ecr.aws/nginx/nginx:latest\n",
        "          Memory: 256\n",
        "  AppService:\n",
        "    Type: AWS::ECS::Service\n",
        "    Properties:\n",
        "      ServiceName: checkout-service\n",
        "      Cluster: !Ref AppCluster\n",
        "      TaskDefinition: !Ref AppTask\n",
        "      DesiredCount: 1\n",
        "Outputs:\n",
        "  ClusterName:\n",
        "    Value: !Ref AppCluster\n",
        "  ServiceArn:\n",
        "    Value: !Ref AppService\n",
        "  ServiceName:\n",
        "    Value: !GetAtt AppService.Name\n",
    );
    create_stack(&router, case, "checkout-stack", template).await;
    let described = stack_once_settled(&router, case, "checkout-stack").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to have completed; status reason {:?}",
        xml_text(&described, "StackStatusReason")
    );

    let outputs = outputs_of(&described);
    assert_eq!(
        outputs.get("ClusterName").map(String::as_str),
        Some("checkout-cluster"),
        "case {case:?}: expected the cluster the template named, got {outputs:?}"
    );
    assert_eq!(
        outputs.get("ServiceName").map(String::as_str),
        Some("checkout-service"),
        "case {case:?}: expected the service the template named, got {outputs:?}"
    );
    assert_eq!(
        outputs.get("ServiceArn").map(String::as_str),
        Some(
            format!("arn:aws:ecs:{REGION}:{ACCOUNT_ID}:service/checkout-cluster/checkout-service")
                .as_str()
        ),
        "case {case:?}: expected the service ARN to carry the cluster hosting it, got \
         {outputs:?}"
    );

    // The cluster segment of that ARN names a cluster Amazon ECS really holds, which is
    // what makes the reported ARN usable against the service that owns it.
    let (status, described_cluster) = ecs(
        &router,
        "DescribeClusters",
        &json!({ "clusters": ["checkout-cluster"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ecs DescribeClusters to answer 200, got {status} with body \
         {described_cluster}"
    );
    assert_eq!(
        described_cluster["clusters"][0]["clusterName"].as_str(),
        Some("checkout-cluster"),
        "case {case:?}: expected Amazon ECS to hold the cluster the service ARN names, got \
         {described_cluster}"
    );
}
