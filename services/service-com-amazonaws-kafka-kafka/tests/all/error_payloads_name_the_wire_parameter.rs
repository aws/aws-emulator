// SPDX-License-Identifier: CC0-1.0

//! WHICH NAME an MSK error payload gives the request parameter at fault, driven end
//! to end over the same `MultiServiceRouter` the emulator serves from (the shared
//! `kafka_harness`).
//!
//! RULE under test: whenever this service populates `invalidParameter` on
//! `BadRequestException`, `NotFoundException` or `ConflictException`, the token it
//! carries is the name the API publishes for that parameter ON THE WIRE — the
//! member's modelled `jsonName` for a body member, the `httpQuery` name for a query
//! member, and the camelCase name the operation's reference documentation gives a
//! path label (`clusterArn`, `arn`, `vpcConnectionArn`) — never the PascalCase
//! Smithy structure-member identifier. The parameter named is also the one the
//! service BLAMES: where a reference names a resource UNDER another (a cluster
//! operation is a record of work done on a cluster), the member reported is the one
//! that carried the reference the lookup actually missed — the cluster where the ARN
//! names an account or a Region this endpoint does not serve, so no record for it
//! could ever live here, and the operation reference itself where it names this
//! endpoint's own.
//!
//! The rows differ along what the rule generalises over: where the value travelled
//! (a non-greedy path label, a query parameter, a JSON body member), which error
//! shape carries the refusal (HTTP 400 `BadRequestException`, HTTP 404
//! `NotFoundException`), what made the request refusable (a reference that resolves
//! to nothing, a value the member cannot carry, a body naming nothing to change, a
//! precondition the stored resource does not meet) and which operation answered (a
//! reader, an update, a delete, a policy write and a Topic-Management call across
//! five resource families).
//!
//! Every row also pins the defect this suite exists for: the token must differ from
//! the Smithy member identifier, which is that same name with its first letter
//! capitalised.
//!
//! The wire names themselves come from the service's own vocabulary where it
//! publishes one ([`service_com_amazonaws_kafka_kafka::state`]), so the suite and
//! the handlers cannot drift apart on how one member is spelled.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::configuration_arn::CONFIGURATION_ARN_MEMBER;
use service_com_amazonaws_kafka_kafka::state::{
    ASSESSMENT_ARN_MEMBER, CHANNEL_ARN_MEMBER, CLUSTER_ARN_MEMBER, CLUSTER_OPERATION_ARN_MEMBER,
    CLUSTER_POLICY_MEMBER, CURRENT_VERSION_MEMBER, NEXT_TOKEN_MEMBER, TOPIC_NAME_MEMBER,
    VPC_CONNECTION_ARN_MEMBER,
};

use crate::kafka_harness::{
    pct, poll_until_active, ClusterRequest, Emulator, ACCOUNT, REGION,
};

/// A cluster id of the form this service mints, for the ARNs below that name
/// resources no registry of this emulator holds.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// An account other than the one this emulator serves, and a Region other than the
/// one the rows are signed for: a record of work on a cluster could only live here
/// for the account and Region this endpoint serves, so an operation ARN naming
/// either of these refers to a cluster no registry here could hold.
const FOREIGN_ACCOUNT: &str = "210987654321";
const OTHER_REGION: &str = "eu-west-1";

/// A version token of the form this service mints that no cluster of this registry
/// is at, for the row whose precondition cannot be met.
const SPENT_VERSION: &str = "K0000000000000";

/// One row: the request to make and the payload it must answer with.
struct Row {
    /// What this row exercises, named in every failure message.
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The HTTP status the refusal carries.
    status: u16,
    /// The parameter the payload must name, by its WIRE name.
    parameter: &'static str,
}

/// The Smithy structure-member identifier for `wire`: the same name with its first
/// letter capitalised, which is the token this service must never put on the wire.
fn member_identifier(wire: &str) -> String {
    let mut characters = wire.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// RULE: every refusal of this service names the parameter at fault by the name the
/// API publishes for it on the wire, and blames the parameter whose resource the
/// service could not act on.
#[tokio::test]
async fn every_refusal_names_the_parameter_by_its_wire_name() {
    let emulator = Emulator::new();

    // A cluster this endpoint really holds, so the rows about a BODY member or a
    // sub-resource reach the member rather than stopping at the cluster.
    let live = emulator
        .create_ok(&ClusterRequest::named("wire-parameter-names"))
        .await;
    let version = poll_until_active(&emulator, &live, "the created cluster").await;

    // A well-formed cluster ARN of this account that this endpoint holds no cluster
    // under, and the sub-resource references of that cluster and of the live one.
    let absent_cluster = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/{GHOST_ID}");
    // Cluster-operation ARNs this endpoint holds no record for. A record of work on
    // a cluster could only live here for the account and the Region this endpoint
    // serves, so the two foreign ones refer to a CLUSTER no registry here could
    // hold, while the own-account, own-Region one names an operation that could have
    // been recorded and was not.
    let foreign_account_operation = format!(
        "arn:aws:kafka:{REGION}:{FOREIGN_ACCOUNT}:cluster-operation/absent/{GHOST_ID}/op-1"
    );
    let foreign_region_operation = format!(
        "arn:aws:kafka:{OTHER_REGION}:{ACCOUNT}:cluster-operation/absent/{GHOST_ID}/op-1"
    );
    let absent_operation =
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/absent/{GHOST_ID}/op-1");
    let absent_configuration =
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/absent/{GHOST_ID}");
    let absent_vpc_connection =
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:vpc-connection/{ACCOUNT}/absent/{GHOST_ID}");
    // A channel and an assessment of the LIVE cluster that it holds no record of:
    // both are sub-resources named under their cluster, so the ARN carries the
    // cluster's own name and id.
    let live_tail = live
        .split_once(":cluster/")
        .map(|(_, tail)| tail.to_string())
        .unwrap_or_default();
    let absent_channel = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/{live_tail}/absent");
    let absent_assessment =
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:assessment/{live_tail}/absent");
    // A cluster-operation ARN of the LIVE cluster naming an operation it never
    // filed, which the V2 reader answers the same way.
    let absent_operation_of_live =
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/{live_tail}/absent-op");

    let cluster_uri = |arn: &str, suffix: &str| format!("/v1/clusters/{}{suffix}", pct(arn));

    let rows = vec![
        // A reference carried in a non-greedy path label, on the resource families
        // that report a reference resolving to nothing as a missing resource.
        Row {
            case: "DescribeCluster on a cluster ARN this endpoint holds nothing under",
            method: "GET",
            uri: cluster_uri(&absent_cluster, ""),
            body: None,
            status: 404,
            parameter: CLUSTER_ARN_MEMBER,
        },
        Row {
            case: "UpdateClusterKafkaVersion on a cluster ARN this endpoint holds nothing under",
            method: "PUT",
            uri: cluster_uri(&absent_cluster, "/version"),
            body: Some(json!({
                "currentVersion": version,
                "targetKafkaVersion": "3.6.0",
            })),
            status: 404,
            parameter: CLUSTER_ARN_MEMBER,
        },
        Row {
            case: "DescribeClusterOperation on an operation ARN of another account",
            method: "GET",
            uri: format!("/v1/operations/{}", pct(&foreign_account_operation)),
            body: None,
            status: 404,
            parameter: CLUSTER_ARN_MEMBER,
        },
        Row {
            case: "DescribeClusterOperation on an operation ARN of another Region",
            method: "GET",
            uri: format!("/v1/operations/{}", pct(&foreign_region_operation)),
            body: None,
            status: 404,
            parameter: CLUSTER_ARN_MEMBER,
        },
        Row {
            case: "DescribeClusterOperation on an operation of this endpoint never recorded",
            method: "GET",
            uri: format!("/v1/operations/{}", pct(&absent_operation)),
            body: None,
            status: 404,
            parameter: CLUSTER_OPERATION_ARN_MEMBER,
        },
        Row {
            case: "DescribeClusterOperation on an operation the live cluster never filed",
            method: "GET",
            uri: format!("/v1/operations/{}", pct(&absent_operation_of_live)),
            body: None,
            status: 404,
            parameter: CLUSTER_OPERATION_ARN_MEMBER,
        },
        Row {
            case: "DescribeClusterOperationV2 on an operation the live cluster never filed",
            method: "GET",
            uri: format!("/api/v2/operations/{}", pct(&absent_operation_of_live)),
            body: None,
            status: 404,
            parameter: CLUSTER_OPERATION_ARN_MEMBER,
        },
        Row {
            case: "DescribeClusterOperationV2 on an operation ARN of another account",
            method: "GET",
            uri: format!("/api/v2/operations/{}", pct(&foreign_account_operation)),
            body: None,
            status: 404,
            parameter: CLUSTER_ARN_MEMBER,
        },
        Row {
            case: "DescribeTopic on a topic the live cluster does not hold",
            method: "GET",
            uri: cluster_uri(&live, "/topics/absent-topic"),
            body: None,
            status: 404,
            parameter: TOPIC_NAME_MEMBER,
        },
        Row {
            case: "DescribeChannel on a channel the live cluster does not hold",
            method: "GET",
            uri: cluster_uri(&live, &format!("/channels/{}", pct(&absent_channel))),
            body: None,
            status: 404,
            parameter: CHANNEL_ARN_MEMBER,
        },
        // The configuration family, whose unresolvable reference is a BAD REQUEST
        // rather than a missing resource, blaming the member its own label carries.
        Row {
            case: "DescribeConfiguration on a configuration ARN that resolves to nothing",
            method: "GET",
            uri: format!("/v1/configurations/{}", pct(&absent_configuration)),
            body: None,
            status: 400,
            parameter: CONFIGURATION_ARN_MEMBER,
        },
        Row {
            case: "DescribeConfigurationRevision on a configuration ARN that resolves to nothing",
            method: "GET",
            uri: format!(
                "/v1/configurations/{}/revisions/1",
                pct(&absent_configuration)
            ),
            body: None,
            status: 400,
            parameter: CONFIGURATION_ARN_MEMBER,
        },
        // The VPC-connection readers bind their label as `Arn` and blame the
        // connection by the name the API documents it under — on the value this
        // service cannot read as a connection reference (a bad request) and on one
        // it reads and holds nothing under (a missing resource) alike.
        Row {
            case: "DescribeVpcConnection on a value that is no connection reference",
            method: "GET",
            uri: format!("/v1/vpc-connection/{}", pct(&live)),
            body: None,
            status: 400,
            parameter: VPC_CONNECTION_ARN_MEMBER,
        },
        Row {
            case: "DeleteVpcConnection on a value that is no connection reference",
            method: "DELETE",
            uri: format!("/v1/vpc-connection/{}", pct(&live)),
            body: None,
            status: 400,
            parameter: VPC_CONNECTION_ARN_MEMBER,
        },
        Row {
            case: "DescribeVpcConnection on a connection ARN that resolves to nothing",
            method: "GET",
            uri: format!("/v1/vpc-connection/{}", pct(&absent_vpc_connection)),
            body: None,
            status: 404,
            parameter: VPC_CONNECTION_ARN_MEMBER,
        },
        Row {
            case: "DeleteVpcConnection on a connection ARN that resolves to nothing",
            method: "DELETE",
            uri: format!("/v1/vpc-connection/{}", pct(&absent_vpc_connection)),
            body: None,
            status: 404,
            parameter: VPC_CONNECTION_ARN_MEMBER,
        },
        Row {
            case: "DescribeAssessment on an assessment the live cluster does not hold",
            method: "GET",
            uri: cluster_uri(&live, &format!("/assessments/{}", pct(&absent_assessment))),
            body: None,
            status: 404,
            parameter: ASSESSMENT_ARN_MEMBER,
        },
        // A reference carried in a QUERY parameter rather than in the path.
        Row {
            case: "GetCompatibleKafkaVersions on a cluster ARN this endpoint holds nothing under",
            method: "GET",
            uri: format!(
                "/v1/compatible-kafka-versions?clusterArn={}",
                pct(&absent_cluster)
            ),
            body: None,
            status: 404,
            parameter: CLUSTER_ARN_MEMBER,
        },
        Row {
            case: "ListClusters resuming from a token this service did not mint",
            method: "GET",
            uri: "/v1/clusters?nextToken=not-a-token".to_string(),
            body: None,
            status: 400,
            parameter: NEXT_TOKEN_MEMBER,
        },
        // BODY members, on the cluster this endpoint holds, so each row reaches the
        // member rather than the target.
        Row {
            case: "UpdateConnectivity naming nothing to change",
            method: "PUT",
            uri: cluster_uri(&live, "/connectivity"),
            body: Some(json!({ "currentVersion": version })),
            status: 400,
            parameter: "connectivityInfo",
        },
        Row {
            case: "UpdateMonitoring naming nothing to change",
            method: "PUT",
            uri: cluster_uri(&live, "/monitoring"),
            body: Some(json!({ "currentVersion": version })),
            status: 400,
            parameter: "enhancedMonitoring",
        },
        Row {
            case: "PutClusterPolicy with a document that is no policy",
            method: "PUT",
            uri: cluster_uri(&live, "/policy"),
            body: Some(json!({ "policy": "not-a-policy-document" })),
            status: 400,
            parameter: CLUSTER_POLICY_MEMBER,
        },
        Row {
            case: "RebootBroker naming a broker the cluster does not hold",
            method: "PUT",
            uri: cluster_uri(&live, "/reboot-broker"),
            body: Some(json!({ "brokerIds": ["404"] })),
            status: 400,
            parameter: "brokerIds",
        },
        Row {
            case: "CreateTopic asking for more replicas than the cluster has brokers",
            method: "POST",
            uri: cluster_uri(&live, "/topics"),
            body: Some(json!({
                "topicName": "past-the-brokers",
                "partitionCount": 1,
                "replicationFactor": 99,
            })),
            status: 400,
            parameter: "replicationFactor",
        },
        Row {
            case: "UpdateStorage quoting a version the cluster is not at",
            method: "PUT",
            uri: cluster_uri(&live, "/storage"),
            body: Some(json!({ "currentVersion": SPENT_VERSION, "volumeSizeGB": 2000 })),
            status: 400,
            parameter: CURRENT_VERSION_MEMBER,
        },
    ];

    for row in rows {
        let reply = emulator.call(row.method, &row.uri, row.body.clone()).await;
        assert_eq!(
            reply.status, row.status,
            "case {:?}: expected HTTP {}, got {} {}",
            row.case, row.status, reply.status, reply.body
        );
        let named = reply.at("/invalidParameter").as_str();
        assert_eq!(
            named,
            Some(row.parameter),
            "case {:?}: expected invalidParameter {:?}, got {:?}",
            row.case,
            row.parameter,
            reply.at("/invalidParameter")
        );
        // The defect this rule is about: the Smithy member identifier is what the
        // payload must NOT carry.
        let identifier = member_identifier(row.parameter);
        assert_ne!(
            named,
            Some(identifier.as_str()),
            "case {:?}: the payload must not name the Smithy member {identifier:?}, got {}",
            row.case,
            reply.body
        );
    }
}
