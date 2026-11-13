// SPDX-License-Identifier: CC0-1.0

//! Amazon MSK resolves the SUBNETS a create would place a resource in against Amazon EC2,
//! which owns them.
//!
//! RULE: a subnet is Amazon EC2's resource, not Amazon MSK's, so every subnet id an Amazon
//! MSK create names as a placement is resolved against the account's EC2 inventory BEFORE
//! anything is minted — a create naming an id EC2 holds no subnet for is refused with the
//! modelled `BadRequestException` (HTTP 400) naming the member at fault and carrying MSK's
//! own fixed sentence for a client subnet it could not resolve (no id of the request is
//! echoed back), writes no record and mints no ARN, while a create naming EC2's own subnets
//! is accepted and the resource it mints is the one the readers then report.
//!
//! This suite lives beside the router because the subnets are EC2's: the VPC and the
//! subnets below are minted by the real EC2 operations on the SAME engine that serves
//! Amazon MSK, so every id in an accepted request is one EC2 handed out rather than one
//! this suite made up, and every refusal is grounded in what EC2 actually answered.
//!
//! The rows differ along what the rule generalises over: the OPERATION that names a
//! placement and the member it names it in (`CreateCluster`'s and `CreateClusterV2`'s
//! provisioned `BrokerNodeGroupInfo$ClientSubnets`, `CreateClusterV2`'s serverless
//! `VpcConfig$SubnetIds`, `CreateVpcConnection`'s `ClientSubnets`), the FORM of the value
//! that names no subnet (EC2's 17-hex id form, its older 8-hex form, and a value that is no
//! subnet id at all), how MANY ids the request names, and WHERE in the list the
//! unresolvable one sits.
//!
//! Error source: `smithy_query(query="operation", operation="CreateCluster,CreateClusterV2,
//! CreateVpcConnection")` on the kafka model — `BadRequestException`, `errorKind` client,
//! `httpError` 400.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// What Amazon MSK answers a create whose client subnets EC2 holds none of: one fixed
/// sentence (its apostrophe is U+2019), with no subnet id of the request in it. Written out
/// here rather than read from the service's own constant, so the wording is pinned by this
/// suite instead of by the code under test.
const SUBNETS_ABSENT_MESSAGE: &str =
    "One or more of the specified client subnets don\u{2019}t exist.";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The Apache Kafka version every provisioned create below asks for.
const KAFKA_VERSION: &str = "3.6.0";

/// The broker instance type every provisioned create below asks for.
const INSTANCE_TYPE: &str = "kafka.m5.large";

/// How often a wait below looks again, and how many looks it takes before it gives up: a
/// GENEROUS budget, since what is waited for is a transition the cluster's own record
/// already owes and the only thing a longer wait costs is a slow machine's patience.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
/// See [`POLL_INTERVAL`]: 600 looks at that interval is half a minute of looking.
const POLLS: usize = 600;

/// The cluster `cluster_arn` names, read again until Amazon MSK reports it `ACTIVE` — the
/// state the clock owes it after the create — or the budget above runs out, and answered as
/// the LAST read either way so an assertion reports what MSK really said.
///
/// Waits for the CONDITION rather than for a span: Amazon MSK manages a VPC connection only
/// from a cluster state in which the cluster HAS brokers, and when that holds is the
/// cluster's own business rather than this suite's.
async fn cluster_once_it_is_active(router: &MultiServiceRouter, cluster_arn: &str) -> Reply {
    let uri = format!("/v1/clusters/{}", form_encode(cluster_arn));
    let mut last = kafka(router, "GET", &uri, None).await;
    for _ in 0..POLLS {
        // A read that is not answered at all is not a transient condition — the ARN is the
        // one the create just minted — so it is answered straight back.
        if last.status != 200
            || last.body.pointer("/clusterInfo/state").and_then(Value::as_str) == Some("ACTIVE")
        {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = kafka(router, "GET", &uri, None).await;
    }
    last
}

/// One emulator carrying Amazon EC2 and Amazon MSK over ONE shared engine, as the
/// production router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_kafka_kafka::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which is
/// what routes the request here; the signature is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
         /aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One answer: its HTTP status and its body as JSON (`Value::Null` when it is not JSON).
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The wire error code (`__type` of the restJson1 error envelope), or `""` on success.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The error envelope's message, or `""` when there is none.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The member the refusal names at fault, or `""` when it names none.
    fn invalid_parameter(&self) -> &str {
        self.body
            .get("invalidParameter")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// Call one Amazon MSK operation over its restJson1 wire.
async fn kafka(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Reply {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("kafka.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("kafka"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    Reply { status, body }
}

/// One ec2Query action that must succeed, answered with its XML body.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("ec2"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {text}"
    );
    text
}

/// The network every case runs over: one VPC with three subnets, one per Availability Zone,
/// all minted by EC2 itself.
struct Network {
    vpc: String,
    subnets: Vec<String>,
}

/// Create that network through EC2's own operations.
async fn ec2_network(router: &MultiServiceRouter) -> Network {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", "10.80.0.0/16")]).await,
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (cidr, zone) in [
        ("10.80.1.0/24", "us-west-2a"),
        ("10.80.2.0/24", "us-west-2b"),
        ("10.80.3.0/24", "us-west-2c"),
    ] {
        subnets.push(element(
            &ec2(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", cidr),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
            "subnetId",
        ));
    }
    Network { vpc, subnets }
}

/// A `CreateCluster` body placing `brokers` brokers in `subnets`.
fn v1_body(name: &str, brokers: u32, subnets: &[String]) -> Value {
    json!({
        "clusterName": name,
        "kafkaVersion": KAFKA_VERSION,
        "numberOfBrokerNodes": brokers,
        "brokerNodeGroupInfo": {
            "instanceType": INSTANCE_TYPE,
            "clientSubnets": subnets,
        },
    })
}

/// A `CreateClusterV2` body whose `Provisioned` block places `brokers` brokers in `subnets`.
fn v2_provisioned_body(name: &str, brokers: u32, subnets: &[String]) -> Value {
    json!({
        "clusterName": name,
        "provisioned": {
            "kafkaVersion": KAFKA_VERSION,
            "numberOfBrokerNodes": brokers,
            "brokerNodeGroupInfo": {
                "instanceType": INSTANCE_TYPE,
                "clientSubnets": subnets,
            },
        },
    })
}

/// A `CreateClusterV2` body whose `Serverless` block carries one VPC configuration per
/// entry of `vpc_configs`.
fn v2_serverless_body(name: &str, vpc_configs: &[Vec<String>]) -> Value {
    json!({
        "clusterName": name,
        "serverless": {
            "vpcConfigs": vpc_configs
                .iter()
                .map(|subnets| json!({ "subnetIds": subnets }))
                .collect::<Vec<Value>>(),
        },
    })
}

/// A `CreateVpcConnection` body reaching `cluster` from `subnets`.
///
/// Amazon MSK serves a connection only for a client-authentication scheme the target
/// cluster turns on, so the cluster this body names is created with SASL/IAM on
/// ([`v1_body_with_sasl_iam`]).
fn connection_body(cluster: &str, vpc: &str, subnets: &[String]) -> Value {
    json!({
        "targetClusterArn": cluster,
        "authentication": "SASL_IAM",
        "vpcId": vpc,
        "clientSubnets": subnets,
        "securityGroups": ["sg-0a1b2c3d"],
    })
}

/// The same `CreateCluster` body with SASL/IAM client authentication turned on — the
/// scheme the VPC connection below asks for, which Amazon MSK requires the target cluster
/// to turn on before it serves the connection.
fn v1_body_with_sasl_iam(name: &str, brokers: u32, subnets: &[String]) -> Value {
    let mut body = v1_body(name, brokers, subnets);
    body["clientAuthentication"] = json!({ "sasl": { "iam": { "enabled": true } } });
    body
}

/// Create a cluster through `CreateCluster`, assert it succeeded, and answer its ARN.
async fn create_v1_ok(router: &MultiServiceRouter, body: Value, case: &str) -> String {
    let reply = kafka(router, "POST", "/v1/clusters", Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: a create placed in EC2's own subnets must be accepted, got {} with {}",
        reply.status, reply.body
    );
    reply
        .body
        .get("clusterArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the create reported no clusterArn: {}", reply.body))
        .to_string()
}

/// Every cluster name `ListClustersV2` reports, over every page.
async fn listed_cluster_names(router: &MultiServiceRouter) -> Vec<String> {
    let mut names = Vec::new();
    let mut uri = "/api/v2/clusters".to_string();
    loop {
        let reply = kafka(router, "GET", &uri, None).await;
        assert_eq!(
            reply.status, 200,
            "a cluster listing must answer 200, got {} with {}",
            reply.status, reply.body
        );
        for cluster in reply
            .body
            .get("clusterInfoList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(name) = cluster.get("clusterName").and_then(Value::as_str) {
                names.push(name.to_string());
            }
        }
        match reply.body.get("nextToken").and_then(Value::as_str) {
            Some(token) => uri = format!("/api/v2/clusters?nextToken={}", form_encode(token)),
            None => break,
        }
    }
    names
}

/// How many VPC connections `ListVpcConnections` reports.
async fn listed_connection_count(router: &MultiServiceRouter) -> usize {
    let reply = kafka(router, "GET", "/v1/vpc-connections", None).await;
    assert_eq!(
        reply.status, 200,
        "a VPC-connection listing must answer 200, got {} with {}",
        reply.status, reply.body
    );
    reply
        .body
        .get("vpcConnections")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// Values that name no subnet of the account, whatever operation names them: EC2's 17-hex
/// id form, its older 8-hex form, and a value that is no subnet id at all. The flag says
/// whether the value is SHAPED like a subnet id, which is what decides whether an operation
/// that reads that shape itself (`CreateVpcConnection`) reaches the resolution gate at all.
fn unresolvable_subnet_ids() -> Vec<(&'static str, String, bool)> {
    vec![
        (
            "EC2's 17-hex id form",
            "subnet-0f1e2d3c4b5a69780".to_string(),
            true,
        ),
        (
            "EC2's older 8-hex id form",
            "subnet-1a2b3c4d".to_string(),
            true,
        ),
        (
            "a value that is no subnet id at all",
            "not-a-subnet".to_string(),
            false,
        ),
    ]
}

/// One row of the refusal table: the case, the request to make, the member the refusal must
/// name at fault, and the cluster name the refused create must leave unregistered (`None`
/// for a request that would mint no cluster).
struct RefusalRow {
    case: String,
    method: &'static str,
    uri: &'static str,
    body: Value,
    parameter: &'static str,
    cluster_name: Option<String>,
    /// The sentence the refusal must carry.
    message: String,
    /// The id the request named that EC2 holds no subnet for. MSK does not echo it back
    /// from the resolution gate, so a refusal carrying
    /// [`SUBNETS_ABSENT_MESSAGE`] must NOT name it.
    unknown: String,
}

/// A `CreateReplicator` body replicating between `source` and `target` (Amazon MSK cluster
/// ARNs), with the replicator's own client interfaces placed in `subnets`.
fn replicator_body(name: &str, source: &str, target: &str, subnets: &[String]) -> Value {
    json!({
        "replicatorName": name,
        "serviceExecutionRoleArn":
            "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole",
        "kafkaClusters": [
            {
                "amazonMskCluster": { "mskClusterArn": source },
                "vpcConfig": { "subnetIds": subnets },
            },
            {
                "amazonMskCluster": { "mskClusterArn": target },
                "vpcConfig": { "subnetIds": subnets },
            },
        ],
        "replicationInfoList": [{
            "sourceKafkaClusterArn": source,
            "targetKafkaClusterArn": target,
            "targetCompressionType": "NONE",
            "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
            "topicReplication": { "topicsToReplicate": [".*"] },
        }],
    })
}

/// RULE: a replicator's own client interfaces are created in the subnets its setup names
/// only once the Amazon MSK clusters that setup names have RESOLVED, so the two references
/// are answered in that order: a setup whose clusters resolve is refused for a subnet EC2
/// holds none of (`BadRequestException` naming that member), while a setup naming a cluster
/// that does not exist is ADMITTED whatever its subnets say and converges to the modelled
/// `FAILED` instead — the cluster is the reason the replicator cannot be provisioned, so
/// the caller is told about that one rather than about a placement MSK never reaches.
#[tokio::test]
async fn a_replicator_s_subnets_are_resolved_once_its_clusters_have() {
    let router = emulator();
    let network = ec2_network(&router).await;
    let source = create_v1_ok(
        &router,
        v1_body("replicator-source", 2, &network.subnets[..2]),
        "replicator-source",
    )
    .await;
    let target = create_v1_ok(
        &router,
        v1_body("replicator-target", 2, &network.subnets[..2]),
        "replicator-target",
    )
    .await;
    let absent = "arn:aws:kafka:us-west-2:000000000000:cluster/replicator-absent\
                  /11111111-2222-4333-8444-555555555555-1";

    // Rows differ along which reference does not resolve: the subnets alone (the clusters
    // are EC2-backed clusters this emulator holds), the cluster alone, and BOTH at once.
    for (index, (form, unknown, _)) in unresolvable_subnet_ids().into_iter().enumerate() {
        let unknown_subnets = vec![network.subnets[0].clone(), unknown.clone()];

        let refused = kafka(
            &router,
            "POST",
            "/replication/v1/replicators",
            Some(replicator_body(
                &format!("subnet-only-{index}"),
                &source,
                &target,
                &unknown_subnets,
            )),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "BadRequestException"),
            "case clusters resolve, subnet unknown in {form}: expected a 400 \
             BadRequestException, got {} with {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.invalid_parameter(),
            "subnetIds",
            "case clusters resolve, subnet unknown in {form}: expected the refusal to name \
             the placement at fault, got {}",
            refused.body
        );
        assert_eq!(
            refused.message(),
            SUBNETS_ABSENT_MESSAGE,
            "case clusters resolve, subnet unknown in {form}: expected the service's fixed \
             sentence, got {}",
            refused.body
        );
        assert!(
            !refused.message().contains(unknown.as_str()),
            "case clusters resolve, subnet unknown in {form}: the refusal must not echo \
             {unknown:?}, got {}",
            refused.body
        );

        // The same request with a cluster that does not exist: admitted, because the
        // cluster reference is the one that decides whether the replicator is provisioned
        // at all. The clock then owes it FAILED — asserted on the kafka service's own
        // suite, which drives the convergence.
        for (variant, (case, cluster_subnets)) in [
            ("held", ("subnets EC2 holds", network.subnets[..2].to_vec())),
            ("unknown", ("subnets EC2 does not", unknown_subnets.clone())),
        ] {
            let admitted = kafka(
                &router,
                "POST",
                "/replication/v1/replicators",
                Some(replicator_body(
                    &format!("absent-cluster-{index}-{variant}"),
                    absent,
                    &target,
                    &cluster_subnets,
                )),
            )
            .await;
            assert_eq!(
                (admitted.status, admitted.error_code()),
                (200, ""),
                "case absent cluster with {case} ({form}): a create whose cluster does not \
                 resolve is admitted, got {} with {}",
                admitted.status,
                admitted.body
            );
            assert_eq!(
                admitted.body.get("replicatorState").and_then(Value::as_str),
                Some("CREATING"),
                "case absent cluster with {case} ({form}): an admitted create reports \
                 CREATING, got {}",
                admitted.body
            );
        }
    }
}

/// Every Amazon MSK create that places a resource in a client VPC resolves the subnets it
/// names against EC2: the ones EC2 holds are accepted and reported back, and every id EC2
/// holds no subnet for is refused with `BadRequestException` while nothing is created.
#[tokio::test]
async fn msk_placement_subnets_resolve_against_ec2() {
    let router = emulator();
    let network = ec2_network(&router).await;

    // The accepted side of the rule, over the four placements: each create names subnets EC2
    // itself minted, so each is accepted — and a provisioned cluster reports the very ids it
    // was placed in.
    let provisioned = create_v1_ok(
        &router,
        v1_body_with_sasl_iam("resolved-v1", 3, &network.subnets),
        "a v1 create placed in EC2's own three subnets",
    )
    .await;
    let described = kafka(
        &router,
        "GET",
        &format!("/v1/clusters/{}", form_encode(&provisioned)),
        None,
    )
    .await;
    assert_eq!(
        described
            .body
            .pointer("/clusterInfo/brokerNodeGroupInfo/clientSubnets")
            .cloned()
            .unwrap_or(Value::Null),
        json!(network.subnets),
        "case \"a v1 create placed in EC2's own three subnets\": expected the cluster to \
         report the subnets it was placed in, got {}",
        described.body
    );

    for (case, body) in [
        (
            "a v2 provisioned create placed in two of EC2's subnets",
            v2_provisioned_body("resolved-v2", 2, &network.subnets[..2]),
        ),
        (
            "a v2 serverless create placed in two of EC2's subnets",
            v2_serverless_body(
                "resolved-serverless",
                &[network.subnets[..2].to_vec()],
            ),
        ),
        (
            "a v2 serverless create with one subnet per VPC configuration",
            v2_serverless_body(
                "resolved-serverless-split",
                &[
                    vec![network.subnets[0].clone()],
                    vec![network.subnets[2].clone()],
                ],
            ),
        ),
    ] {
        let reply = kafka(&router, "POST", "/api/v2/clusters", Some(body)).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a create placed in EC2's own subnets must be accepted, got {} with {}",
            reply.status, reply.body
        );
    }

    // Amazon MSK manages a VPC connection only from a cluster state in which the cluster
    // HAS brokers, so the connection below is made once MSK itself reports this cluster
    // `ACTIVE` ([`cluster_once_it_is_active`] waits for that condition, not for a span). It
    // is reached from as many of EC2's subnets as the cluster itself is placed in, because
    // MSK places one interface per Availability Zone the cluster has brokers in.
    let settled = cluster_once_it_is_active(&router, &provisioned).await;
    assert_eq!(
        settled.body.pointer("/clusterInfo/state").and_then(Value::as_str),
        Some("ACTIVE"),
        "the cluster a VPC connection is made to must reach ACTIVE, last saw {}",
        settled.body
    );
    let connected = kafka(
        &router,
        "POST",
        "/v1/vpc-connection",
        Some(connection_body(&provisioned, &network.vpc, &network.subnets)),
    )
    .await;
    assert_eq!(
        connected.status, 200,
        "case \"a VPC connection reached from EC2's own subnets\": expected the create to \
         be accepted, got {} with {}",
        connected.status, connected.body
    );

    // What the account holds once every accepted create has run: the refusals below must
    // leave exactly this.
    let accepted_names = listed_cluster_names(&router).await;
    let accepted_connections = listed_connection_count(&router).await;

    let mut rows: Vec<RefusalRow> = Vec::new();
    for (form, unknown, shaped) in unresolvable_subnet_ids() {
        let only = vec![unknown.clone()];
        let unknown_last = vec![network.subnets[0].clone(), unknown.clone()];
        let unknown_first = vec![
            unknown.clone(),
            network.subnets[0].clone(),
            network.subnets[1].clone(),
        ];
        rows.push(RefusalRow {
            case: format!("a v1 create placed in one subnet EC2 does not hold, in {form}"),
            method: "POST",
            uri: "/v1/clusters",
            body: v1_body("dead-v1-only", 2, &only),
            parameter: "brokerNodeGroupInfo",
            cluster_name: Some("dead-v1-only".to_string()),
            message: SUBNETS_ABSENT_MESSAGE.to_string(),
            unknown: unknown.clone(),
        });
        rows.push(RefusalRow {
            case: format!(
                "a v1 create naming one subnet EC2 holds and then one it does not, in {form}"
            ),
            method: "POST",
            uri: "/v1/clusters",
            body: v1_body("dead-v1-last", 2, &unknown_last),
            parameter: "brokerNodeGroupInfo",
            cluster_name: Some("dead-v1-last".to_string()),
            message: SUBNETS_ABSENT_MESSAGE.to_string(),
            unknown: unknown.clone(),
        });
        rows.push(RefusalRow {
            case: format!(
                "a v1 create whose FIRST of three subnets EC2 does not hold, in {form}"
            ),
            method: "POST",
            uri: "/v1/clusters",
            body: v1_body("dead-v1-first", 3, &unknown_first),
            parameter: "brokerNodeGroupInfo",
            cluster_name: Some("dead-v1-first".to_string()),
            message: SUBNETS_ABSENT_MESSAGE.to_string(),
            unknown: unknown.clone(),
        });
        rows.push(RefusalRow {
            case: format!(
                "a v2 provisioned create naming a subnet EC2 does not hold, in {form}"
            ),
            method: "POST",
            uri: "/api/v2/clusters",
            body: v2_provisioned_body("dead-v2", 2, &unknown_last),
            parameter: "brokerNodeGroupInfo",
            cluster_name: Some("dead-v2".to_string()),
            message: SUBNETS_ABSENT_MESSAGE.to_string(),
            unknown: unknown.clone(),
        });
        rows.push(RefusalRow {
            case: format!(
                "a v2 serverless create whose SECOND VPC configuration names a subnet EC2 \
                 does not hold, in {form}"
            ),
            method: "POST",
            uri: "/api/v2/clusters",
            body: v2_serverless_body(
                "dead-serverless",
                &[vec![network.subnets[0].clone()], only.clone()],
            ),
            parameter: "vpcConfigs",
            cluster_name: Some("dead-serverless".to_string()),
            message: SUBNETS_ABSENT_MESSAGE.to_string(),
            unknown: unknown.clone(),
        });
        rows.push(RefusalRow {
            case: format!(
                "a VPC connection reached from a subnet EC2 does not hold, in {form}"
            ),
            method: "POST",
            uri: "/v1/vpc-connection",
            // As many subnets as the target cluster is placed in, so what refuses this is
            // the id EC2 does not hold rather than the placement's cardinality.
            body: connection_body(&provisioned, &network.vpc, &unknown_first),
            parameter: "ClientSubnets",
            cluster_name: None,
            // `CreateVpcConnection` reads the SHAPE of each id it is given itself, so a
            // value that is no subnet id at all never reaches the resolution gate: that
            // handler's own sentence for an unusable id is what the caller gets.
            message: match shaped {
                true => SUBNETS_ABSENT_MESSAGE.to_string(),
                false => format!("'{unknown}' is not a valid subnet id."),
            },
            unknown: unknown.clone(),
        });
    }

    for row in &rows {
        let refused = kafka(&router, row.method, row.uri, Some(row.body.clone())).await;
        let (case, unknown) = (&row.case, &row.unknown);
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "BadRequestException"),
            "case {case:?}: expected a 400 BadRequestException, got {} with {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.invalid_parameter(),
            row.parameter,
            "case {case:?}: expected the refusal to name {} at fault, got {}",
            row.parameter,
            refused.body
        );
        assert_eq!(
            refused.message(),
            row.message,
            "case {case:?}: expected {:?}, got {}",
            row.message,
            refused.body
        );
        assert!(
            row.message != SUBNETS_ABSENT_MESSAGE || !refused.message().contains(unknown.as_str()),
            "case {case:?}: the refusal must not echo {unknown:?}, got {}",
            refused.body
        );

        // Nothing was created: no cluster under the refused name is enumerable, and the
        // VPC-connection registry is as it was.
        let names = listed_cluster_names(&router).await;
        assert_eq!(
            names, accepted_names,
            "case {case:?}: a refused create must register no cluster, got {names:?}"
        );
        if let Some(name) = &row.cluster_name {
            assert!(
                !names.contains(name),
                "case {case:?}: the refused create left {name:?} enumerable: {names:?}"
            );
        }
        let connections = listed_connection_count(&router).await;
        assert_eq!(
            connections, accepted_connections,
            "case {case:?}: a refused create must register no VPC connection, got \
             {connections} of them"
        );
    }
}

/// RULE: a create's own REQUEST is admitted before the account's network is asked about
/// it, so a create that is wrong in both ways at once — an Apache Kafka version the
/// service does not publish AND subnets EC2 holds none of — is answered about the
/// version. The order is observable only with EC2 beside Amazon MSK, which is why it is
/// asserted here: without one the placement gate never fires at all.
#[tokio::test]
async fn a_requests_own_rules_are_answered_before_the_subnets_are_resolved() {
    let router = emulator();
    let network = ec2_network(&router).await;

    // Rows differ along the operation (the v1 create and the v2 provisioned one) and
    // along how many of the subnets EC2 holds: none of them, and one of two. Neither
    // changes the answer, because the version is settled first.
    let unknown = "subnet-0f1e2d3c4b5a69780".to_string();
    let subnet_rows = [
        ("no subnet EC2 holds", vec![unknown.clone()]),
        (
            "one subnet EC2 holds and one it does not",
            vec![network.subnets[0].clone(), unknown.clone()],
        ),
    ];

    for (index, (subnet_case, subnets)) in subnet_rows.iter().enumerate() {
        for (operation, uri, body) in [
            (
                "CreateCluster",
                "/v1/clusters",
                v1_body(&format!("order-v1-{index}"), 2, subnets),
            ),
            (
                "CreateClusterV2",
                "/api/v2/clusters",
                v2_provisioned_body(&format!("order-v2-{index}"), 2, subnets),
            ),
        ] {
            let case = format!("{operation} with an unknown version and {subnet_case}");
            let mut body = body;
            // A version the catalog does not publish, in the member the operation
            // carries it in.
            if body.get("provisioned").is_some() {
                body["provisioned"]["kafkaVersion"] = json!("9.9.9");
            } else {
                body["kafkaVersion"] = json!("9.9.9");
            }
            let refused = kafka(&router, "POST", uri, Some(body)).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (400, "BadRequestException"),
                "case {case:?}: expected a 400 BadRequestException, got {} with {}",
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.invalid_parameter(),
                "kafkaVersion",
                "case {case:?}: expected the refusal to be about the version, got {}",
                refused.body
            );
            assert!(
                !refused.message().contains(unknown.as_str())
                    && refused.message() != SUBNETS_ABSENT_MESSAGE,
                "case {case:?}: the subnets must not be resolved at all, got {}",
                refused.body
            );
        }
    }
}
