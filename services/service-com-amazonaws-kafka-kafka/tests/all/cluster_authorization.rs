// SPDX-License-Identifier: CC0-1.0

//! Resource-scoped authorization over the cluster an MSK request names, driven end
//! to end over the same router the emulator serves from.
//!
//! The RULE under test: a cluster-scoped operation decides whether the caller may
//! perform its action on the cluster ARN the request carries BEFORE it resolves
//! that cluster or validates the rest of the body — so a target ARN in an account
//! other than the caller's, which no resource-based policy the caller holds can
//! allow, is refused HTTP 403 with the error code `AccessDeniedException` and the
//! IAM not-authorized-by-resource-policy sentence naming the calling principal, the
//! operation's IAM action and that ARN, whatever the registry does or does not hold
//! under it. The same rule over a role PASSED to the service: `CreateReplicator`
//! accepts a `ServiceExecutionRoleArn` of the caller's account only, and refuses one
//! of another account outright with its own fixed sentence, minting nothing.
//!
//! Each suite asserts that over a TABLE rather than one request. The rows differ
//! along what the rule generalises over: which operation is called (every
//! implemented cluster-scoped one — reads, listings, creates, deletes and updates,
//! on both the `/v1` and the `/api/v2` URI families), which IAM action it is
//! authorized under (`kafka:<ApiName>` for the control plane, `kafka-cluster:Connect`
//! for the Topic Management calls, which reach the cluster's Apache Kafka data
//! plane), how the target ARN travels (an `@httpLabel`, a body member, a query
//! parameter), and the ARN itself (several foreign accounts, another region, another
//! partition, and a resource segment this service never mints).
//!
//! The controls pin the two boundaries: the SAME calls on a cluster ARN of the
//! caller's OWN account are never answered with the denial — they reach the checks
//! and the lookups they always did, and a cluster the caller owns is served — and a
//! request body that is invalid is still refused with the denial when the cluster is
//! foreign, because the decision precedes body validation.
//!
//! The router, the `Reply` wrapper, the percent-encoder and the request builders
//! come from [`crate::kafka_harness`], the crate's shared harness.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::authorization::caller_principal;

use crate::kafka_harness::{pct, sleep_past_activation, ClusterRequest, Emulator, ACCOUNT, REGION};

/// A cluster id of the form this service mints, for an ARN naming a cluster no
/// registry of this emulator holds.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// The HTTP status an authorization refusal carries, as the recorded refusals do;
/// spelled out here rather than read from the service, so a drift in the service's
/// own constant fails this suite.
const ACCESS_DENIED_STATUS: u16 = 403;

/// The wire error code (`x-amzn-errortype`, and the JSON envelope's `__type`) those
/// refusals carry, likewise spelled out.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The whole sentence MSK refuses a role it may not be passed with: fixed text end
/// to end, carrying nothing from the request.
const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// The IAM action a Topic Management call is authorized under: those calls reach the
/// cluster's Apache Kafka data plane, whose actions MSK publishes in its own
/// `kafka-cluster` namespace.
const CLUSTER_CONNECT_ACTION: &str = "kafka-cluster:Connect";

/// Where the ARN a call acts on travels, which is the other thing the rule has to
/// hold over: every form reaches the same decision.
#[derive(Debug, Clone)]
enum Target {
    /// `{base}/{ClusterArn}{suffix}` — the ARN is an `@httpLabel`.
    Path {
        base: &'static str,
        suffix: String,
    },
    /// The ARN travels in the request body, under `member`.
    Body {
        uri: &'static str,
        member: &'static str,
    },
    /// The ARN travels as a query parameter.
    Query {
        uri: &'static str,
        parameter: &'static str,
    },
}

/// One cluster-scoped call: how the model binds it, the body it carries, and the
/// IAM action MSK authorizes it under.
#[derive(Debug, Clone)]
struct ClusterCall {
    case: &'static str,
    method: &'static str,
    target: Target,
    body: Option<Value>,
    action: String,
}

impl ClusterCall {
    /// A control-plane call: its IAM action is `kafka:` plus the API name.
    fn control(case: &'static str, method: &'static str, suffix: &str, body: Option<Value>) -> Self {
        Self {
            case,
            method,
            target: Target::Path {
                base: "/v1/clusters",
                suffix: suffix.to_string(),
            },
            body,
            action: format!("kafka:{case}"),
        }
    }

    /// The same, under the `/api/v2` URI family.
    fn control_v2(case: &'static str, method: &'static str, suffix: &str) -> Self {
        Self {
            target: Target::Path {
                base: "/api/v2/clusters",
                suffix: suffix.to_string(),
            },
            ..Self::control(case, method, "", None)
        }
    }

    /// A Topic Management call: it reaches the cluster's Apache Kafka data plane, so
    /// it is authorized under [`CLUSTER_CONNECT_ACTION`] rather than under a
    /// `kafka:` control-plane action.
    fn topic(case: &'static str, method: &'static str, suffix: &str, body: Option<Value>) -> Self {
        Self {
            action: CLUSTER_CONNECT_ACTION.to_string(),
            ..Self::control(case, method, suffix, body)
        }
    }

    /// The URI and body this call makes against the cluster `cluster_arn` names.
    fn request(&self, cluster_arn: &str) -> (String, Option<Value>) {
        match &self.target {
            Target::Path { base, suffix } => (
                format!("{base}/{}{suffix}", pct(cluster_arn)),
                self.body.clone(),
            ),
            Target::Body { uri, member } => {
                let mut body = self.body.clone().unwrap_or_else(|| json!({}));
                body[*member] = json!(cluster_arn);
                ((*uri).to_string(), Some(body))
            }
            Target::Query { uri, parameter } => (
                format!("{uri}?{parameter}={}", pct(cluster_arn)),
                self.body.clone(),
            ),
        }
    }
}

/// A `CreateChannel` body that is ALSO invalid: it names neither destination
/// configuration, which is what this handler refuses once it is reached. It carries
/// the two members the model requires, so the boundary validator passes it and the
/// handler is what answers.
fn channel_body() -> Value {
    json!({
        "channelName": "orders-channel",
        "topicConfigurationList": [{
            "recordConverter": { "valueConverter": "JSON" },
            "topicArn": format!("arn:aws:kafka:{REGION}:{ACCOUNT}:topic/channels/orders"),
        }],
    })
}

/// Every implemented cluster-scoped call, as a table. The rows span the operation
/// kinds (read, listing, create, delete, update), both URI families, both IAM action
/// namespaces, and all three ways the target ARN travels.
fn cluster_calls() -> Vec<ClusterCall> {
    let secret_arns = json!({
        "secretArnList": [
            format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:AmazonMSK_denied")
        ],
    });
    let channel_arn = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/foreign/abc-1/orders-channel");
    vec![
        ClusterCall::control("DescribeCluster", "GET", "", None),
        ClusterCall::control_v2("DescribeClusterV2", "GET", ""),
        ClusterCall::control("DeleteCluster", "DELETE", "", None),
        ClusterCall::control("GetBootstrapBrokers", "GET", "/bootstrap-brokers", None),
        ClusterCall::control("GetClusterPolicy", "GET", "/policy", None),
        ClusterCall::control(
            "PutClusterPolicy",
            "PUT",
            "/policy",
            Some(json!({ "policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}" })),
        ),
        ClusterCall::control("DeleteClusterPolicy", "DELETE", "/policy", None),
        ClusterCall::control("ListNodes", "GET", "/nodes", None),
        ClusterCall::control("ListScramSecrets", "GET", "/scram-secrets", None),
        ClusterCall::control(
            "BatchAssociateScramSecret",
            "POST",
            "/scram-secrets",
            Some(secret_arns.clone()),
        ),
        ClusterCall::control(
            "BatchDisassociateScramSecret",
            "PATCH",
            "/scram-secrets",
            Some(secret_arns),
        ),
        ClusterCall::control(
            "ListClientVpcConnections",
            "GET",
            "/client-vpc-connections",
            None,
        ),
        ClusterCall::control("ListClusterOperations", "GET", "/operations", None),
        ClusterCall::control(
            "RebootBroker",
            "PUT",
            "/reboot-broker",
            Some(json!({ "brokerIds": ["1"] })),
        ),
        ClusterCall::control("CreateChannel", "POST", "/channels", Some(channel_body())),
        ClusterCall::control(
            "DescribeChannel",
            "GET",
            &format!("/channels/{}", pct(&channel_arn)),
            None,
        ),
        ClusterCall::control(
            "DeleteChannel",
            "DELETE",
            &format!("/channels/{}", pct(&channel_arn)),
            None,
        ),
        ClusterCall::control(
            "UpdateBrokerCount",
            "PUT",
            "/nodes/count",
            Some(json!({
                "currentVersion": "K3AEGXETSR30VB",
                "targetNumberOfBrokerNodes": 4,
            })),
        ),
        ClusterCall::control(
            "UpdateBrokerStorage",
            "PUT",
            "/nodes/storage",
            Some(json!({
                "currentVersion": "K3AEGXETSR30VB",
                "targetBrokerEBSVolumeInfo": [
                    { "kafkaBrokerNodeId": "ALL", "volumeSizeGB": 200 }
                ],
            })),
        ),
        ClusterCall::control(
            "UpdateBrokerType",
            "PUT",
            "/nodes/type",
            Some(json!({
                "currentVersion": "K3AEGXETSR30VB",
                "targetInstanceType": "kafka.m5.xlarge",
            })),
        ),
        ClusterCall::control(
            "UpdateClusterConfiguration",
            "PUT",
            "/configuration",
            Some(json!({
                "currentVersion": "K3AEGXETSR30VB",
                "configurationInfo": {
                    "arn": format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/cfg/abc-1"),
                    "revision": 1,
                },
            })),
        ),
        ClusterCall::control(
            "UpdateClusterKafkaVersion",
            "PUT",
            "/version",
            Some(json!({
                "currentVersion": "K3AEGXETSR30VB",
                "targetKafkaVersion": "3.7.x",
            })),
        ),
        ClusterCall::control(
            "UpdateConnectivity",
            "PUT",
            "/connectivity",
            Some(json!({
                "currentVersion": "K3AEGXETSR30VB",
                "connectivityInfo": { "publicAccess": { "type": "DISABLED" } },
            })),
        ),
        ClusterCall::control(
            "UpdateSecurity",
            "PATCH",
            "/security",
            Some(json!({ "currentVersion": "K3AEGXETSR30VB" })),
        ),
        ClusterCall::control(
            "UpdateStorage",
            "PUT",
            "/storage",
            Some(json!({ "currentVersion": "K3AEGXETSR30VB", "volumeSizeGB": 200 })),
        ),
        ClusterCall::topic(
            "CreateTopic",
            "POST",
            "/topics",
            Some(json!({
                "topicName": "orders",
                "partitionCount": 3,
                "replicationFactor": 1,
            })),
        ),
        ClusterCall::topic(
            "DescribeTopic",
            "GET",
            &format!("/topics/{}", pct("orders")),
            None,
        ),
        ClusterCall::topic(
            "DeleteTopic",
            "DELETE",
            &format!("/topics/{}", pct("orders")),
            None,
        ),
        ClusterCall::topic("ListTopics", "GET", "/topics", None),
        ClusterCall {
            target: Target::Body {
                uri: "/v1/vpc-connection",
                member: "targetClusterArn",
            },
            body: Some(json!({
                "authentication": "SASL_IAM",
                "vpcId": "vpc-0a1b2c3d",
                "clientSubnets": ["subnet-0a1b2c3d"],
                "securityGroups": ["sg-0a1b2c3d"],
            })),
            ..ClusterCall::control("CreateVpcConnection", "POST", "", None)
        },
        ClusterCall {
            target: Target::Query {
                uri: "/v1/compatible-kafka-versions",
                parameter: "clusterArn",
            },
            ..ClusterCall::control("GetCompatibleKafkaVersions", "GET", "", None)
        },
    ]
}

/// The sentence AWS refuses an unauthorized request with, as the recorded refusals
/// word it: fixed text around the calling principal, the IAM action asked for and
/// the resource it was asked for.
fn denial_message(action: &str, resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: {resource} \
         because no resource-based policy allows the {action} action",
        caller_principal()
    )
}

/// Cluster ARNs no caller of this emulator holds a policy over: the rows differ
/// along the account, the region, the partition, and whether the resource segment is
/// even one this service mints — the last showing that the decision is taken on the
/// ARN as submitted, before anything parses it.
fn foreign_cluster_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "another account",
            format!("arn:aws:kafka:{REGION}:111122223333:cluster/foreign/{GHOST_ID}"),
        ),
        (
            "a different other account",
            format!("arn:aws:kafka:{REGION}:210987654321:cluster/other-owner/{GHOST_ID}"),
        ),
        (
            "another account in another region",
            format!("arn:aws:kafka:eu-west-1:999988887777:cluster/far-away/{GHOST_ID}"),
        ),
        (
            "another account in another partition",
            format!("arn:aws-us-gov:kafka:{REGION}:111122223333:cluster/gov-cloud/{GHOST_ID}"),
        ),
        (
            "another account, a resource segment this service never mints",
            format!("arn:aws:kafka:{REGION}:111122223333:cluster/only-a-name"),
        ),
    ]
}

/// The principal a refusal names is the one identity this emulator's account holds,
/// which `sts:GetCallerIdentity` reports for every request — pinned here so the
/// spelling the sentences below are built from is itself asserted.
#[tokio::test]
async fn the_refused_principal_is_the_accounts_one_identity() {
    let expected = format!("arn:aws:iam::{ACCOUNT}:user/emulator");
    assert_eq!(
        caller_principal(),
        expected,
        "case the calling principal: expected {expected:?}, got {:?}",
        caller_principal()
    );
}

/// RULE: every cluster-scoped operation refuses a target cluster ARN in another
/// account with HTTP 403, the `AccessDeniedException` code and the
/// not-authorized-by-resource-policy sentence naming the caller, the operation's IAM
/// action and the submitted ARN — whatever the operation, however the ARN travels,
/// and whether or not the registry holds anything under it.
#[tokio::test]
async fn every_cluster_scoped_operation_refuses_a_foreign_account_cluster() {
    let emulator = Emulator::new();

    for (arn_case, arn) in foreign_cluster_arns() {
        for call in cluster_calls() {
            let (uri, body) = call.request(&arn);
            let reply = emulator.call(call.method, &uri, body).await;
            let case = format!("{} / {arn_case}", call.case);
            assert_eq!(
                reply.status, ACCESS_DENIED_STATUS,
                "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                ACCESS_DENIED_CODE,
                "case {case:?}: expected {ACCESS_DENIED_CODE}, got {:?}",
                reply.error_code()
            );
            assert_eq!(
                reply.error_message(),
                denial_message(&call.action, &arn),
                "case {case:?}: expected the resource-policy denial naming {:?}, got {:?}",
                call.action,
                reply.error_message()
            );
        }
    }
}

/// RULE (the ordering half): the decision is taken before the request body is
/// validated, so a call whose body is ALSO invalid still answers the denial — the
/// body-validation complaint it would otherwise get never reaches the caller.
#[tokio::test]
async fn the_decision_precedes_body_validation() {
    let emulator = Emulator::new();
    let foreign = format!("arn:aws:kafka:{REGION}:111122223333:cluster/foreign/{GHOST_ID}");
    let owned = emulator.create_ok(&ClusterRequest::named("channel-owner")).await;
    sleep_past_activation();

    // The SAME invalid body, against a foreign cluster and against one the caller
    // owns: the first is the authorization refusal, the second the body complaint.
    let refused = emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/channels", pct(&foreign)),
            Some(channel_body()),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_message()),
        (
            ACCESS_DENIED_STATUS,
            denial_message("kafka:CreateChannel", &foreign).as_str()
        ),
        "case a foreign cluster with an invalid body: expected the authorization denial, got {} {}",
        refused.status,
        refused.body
    );

    let complained = emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/channels", pct(&owned)),
            Some(channel_body()),
        )
        .await;
    assert_eq!(
        complained.status, 400,
        "case an owned cluster with an invalid body: expected the body complaint, got {} {}",
        complained.status, complained.body
    );
    assert_ne!(
        complained.error_code(),
        ACCESS_DENIED_CODE,
        "case an owned cluster with an invalid body: expected no authorization denial, got {}",
        complained.body
    );
}

/// RULE (the other boundary): the decision rests on the ARN's ACCOUNT, so a cluster
/// ARN of the caller's own account is never refused by it — the call reaches the
/// checks and the lookups it always did, and a cluster the caller owns is served.
/// What this endpoint holds under such an ARN is then each operation's own to answer.
#[tokio::test]
async fn a_cluster_arn_of_the_callers_own_account_is_never_denied() {
    let emulator = Emulator::new();
    let absent = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/{GHOST_ID}");

    for call in cluster_calls() {
        let (uri, body) = call.request(&absent);
        let reply = emulator.call(call.method, &uri, body).await;
        assert_ne!(
            reply.status, ACCESS_DENIED_STATUS,
            "case {}: an ARN of the caller's own account must not be refused as unauthorized, \
             got {} {}",
            call.case, reply.status, reply.body
        );
        assert_ne!(
            reply.error_code(),
            ACCESS_DENIED_CODE,
            "case {}: an ARN of the caller's own account must not be refused as unauthorized, \
             got {}",
            call.case,
            reply.body
        );
    }

    // And the gate lets a real request through: a cluster this caller created reads
    // back through both Describe versions.
    let owned = emulator.create_ok(&ClusterRequest::named("owned-cluster")).await;
    sleep_past_activation();
    for (case, reply) in [
        ("DescribeCluster", emulator.describe(&owned).await),
        ("DescribeClusterV2", emulator.describe_v2(&owned).await),
    ] {
        assert_eq!(
            reply.status, 200,
            "case {case}: expected the owned cluster to be served, got {} {}",
            reply.status, reply.body
        );
    }
}

/// `CreateReplicator`'s body, with `ServiceExecutionRoleArn` as the row gives it.
fn replicator_body(name: &str, role_arn: &str) -> Value {
    let source = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/{name}-source/{GHOST_ID}");
    let target = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/{name}-target/{GHOST_ID}");
    json!({
        "replicatorName": name,
        "serviceExecutionRoleArn": role_arn,
        "kafkaClusters": [
            {
                "amazonMskCluster": { "mskClusterArn": source },
                "vpcConfig": {
                    "subnetIds": ["subnet-aaaaaaaaaaaaaaaaa", "subnet-bbbbbbbbbbbbbbbbb"],
                },
            },
            {
                "amazonMskCluster": { "mskClusterArn": target },
                "vpcConfig": {
                    "subnetIds": ["subnet-aaaaaaaaaaaaaaaaa", "subnet-bbbbbbbbbbbbbbbbb"],
                },
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

/// RULE: a role is PASSED to this service, and only a role of the caller's own
/// account may be passed — so `CreateReplicator` refuses a `ServiceExecutionRoleArn`
/// that is not an IAM role ARN of this account with HTTP 403 `AccessDeniedException`
/// and its own fixed sentence, and mints nothing; a role of the caller's account is
/// admitted.
#[tokio::test]
async fn create_replicator_refuses_a_cross_account_service_execution_role() {
    let emulator = Emulator::new();

    // The rows differ along WHY the value is not a role of the caller's account:
    // another account, another account under a path, another partition, an ARN of
    // another identity kind or another service in this very account, and text no
    // account can be read out of at all.
    let refused: Vec<(&str, String)> = vec![
        (
            "another account",
            "arn:aws:iam::111122223333:role/MSKReplicatorRole".to_string(),
        ),
        (
            "a different other account",
            "arn:aws:iam::210987654321:role/service-role/MSKReplicator".to_string(),
        ),
        (
            "another account in another partition",
            "arn:aws-us-gov:iam::999988887777:role/MSKReplicatorRole".to_string(),
        ),
        (
            "this account's IAM user rather than a role",
            format!("arn:aws:iam::{ACCOUNT}:user/emulator"),
        ),
        (
            "an ARN of another service in this account",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/not-a-role/{GHOST_ID}"),
        ),
        ("not an ARN at all", "not-a-role-arn".to_string()),
        ("a truncated ARN", "arn:aws:iam".to_string()),
        (
            // IAM is partition-global, so an IAM ARN carries no Region: a value with
            // one names no role IAM holds.
            "an IAM ARN carrying a Region",
            format!("arn:aws:iam:{REGION}:{ACCOUNT}:role/MSKReplicatorRole"),
        ),
    ];

    for (index, (case, role_arn)) in refused.iter().enumerate() {
        let name = format!("crossaccount{index}");
        let reply = emulator
            .call(
                "POST",
                "/replication/v1/replicators",
                Some(replicator_body(&name, role_arn)),
            )
            .await;
        assert_eq!(
            reply.status, ACCESS_DENIED_STATUS,
            "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            ACCESS_DENIED_CODE,
            "case {case:?}: expected {ACCESS_DENIED_CODE}, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            reply.error_message(),
            CROSS_ACCOUNT_PASS_ROLE_MESSAGE,
            "case {case:?}: expected the fixed cross-account pass-role refusal, got {:?}",
            reply.error_message()
        );
        // The refusal minted nothing: the replicator the call named is in no
        // listing.
        let listed = emulator.call("GET", "/replication/v1/replicators", None).await;
        assert!(
            !listed.body.to_string().contains(&name),
            "case {case:?}: a refused create must register no replicator, but {name:?} is in {}",
            listed.body
        );
    }

    // The same request with a role of the caller's own account is admitted, so the
    // refusal rests on the role's account and on nothing else about the request.
    for (case, role_arn) in [
        ("own account", format!("arn:aws:iam::{ACCOUNT}:role/MSKReplicatorRole")),
        (
            "own account under a path",
            format!("arn:aws:iam::{ACCOUNT}:role/service-role/MSKReplicator"),
        ),
        // An ARN naming NO account names no other account either, so the role it
        // names is one this account's IAM can be asked about.
        (
            "no account segment",
            "arn:aws:iam:::role/MSKReplicatorRole".to_string(),
        ),
    ] {
        let name = format!("ownaccount{}", case.len());
        let reply = emulator
            .call(
                "POST",
                "/replication/v1/replicators",
                Some(replicator_body(&name, &role_arn)),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected a role of the caller's account to be admitted, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/replicatorState").as_str(),
            Some("CREATING"),
            "case {case:?}: an admitted create reports CREATING, got {}",
            reply.body
        );
    }
}
