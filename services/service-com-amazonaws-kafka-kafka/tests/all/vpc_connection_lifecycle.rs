// SPDX-License-Identifier: CC0-1.0

//! The MSK VPC-connection lifecycle (`CreateVpcConnection`,
//! `DescribeVpcConnection`, `DeleteVpcConnection`), driven end to end over the
//! wire through the crate's shared harness.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request:
//! the rows differ along whatever the rule generalises over (authentication
//! type, VPC/subnet/security-group id form, subnet and security-group count,
//! tag count, target cluster, signing region, connection count). Where the rule
//! is an invariant — a created connection reading back whole, a refused create
//! registering nothing, each connection getting its own ARN — the input space is
//! walked exhaustively over the published value set rather than sampled.

use serde_json::json;
use service_com_amazonaws_kafka_kafka::state::{
    BAD_REQUEST_MESSAGE, VPC_CONNECTION_AUTHENTICATION_TYPES,
};

use crate::kafka_harness::{
    sleep_past_vpc_connection_activation, vpc_connection_uri, ConnectionRequest, Emulator, ACCOUNT,
    REGION,
};

/// Every member `DescribeVpcConnectionResponse` declares, so a suite can assert
/// that the reader reports all of them rather than merely the ones it checks.
const DESCRIBE_MEMBERS: &[&str] = &[
    "vpcConnectionArn",
    "targetClusterArn",
    "state",
    "authentication",
    "vpcId",
    "subnets",
    "securityGroups",
    "creationTime",
    "tags",
];

/// A provisioned cluster in this suite's signing region that Amazon MSK admits a
/// VPC connection to — settled, and with every client authentication a connection
/// may ask for turned on, which is what MSK requires of the target. A VPC
/// connection is always made to one, so every suite below needs such a target;
/// the preconditions themselves are the subject of
/// `vpc_connection_admission.rs`.
async fn cluster(emulator: &Emulator, name: &str) -> String {
    emulator.vpc_connection_target(name).await
}

/// RULE: a created VPC connection is persisted whole, and `DescribeVpcConnection`
/// reports every member from that record — the submitted values where the
/// request carried them, the ARN and creation instant the service minted, and
/// the state the clock has reached.
#[tokio::test]
async fn describe_reports_exactly_what_was_created() {
    let emulator = Emulator::new();
    let target_one = cluster(&emulator, "target-one").await;
    let target_two = cluster(&emulator, "target-two").await;

    // The rows differ along everything the rule generalises over: every
    // authentication type the service publishes, both published EC2 id lengths,
    // subnet ORDER, security-group count, tag count, and the target cluster. The
    // subnet COUNT is not free: a connection spans as many client subnets as its
    // target cluster.
    let mut rows = vec![
        ConnectionRequest::to(&target_one),
        ConnectionRequest {
            authentication: "SASL_SCRAM",
            // The 17-hex-digit EC2 id form.
            vpc_id: "vpc-0123456789abcdef0",
            client_subnets: vec![
                "subnet-0123456789abcdef0",
                "subnet-0a1b2c3e",
                "subnet-0a1b2c3f",
            ],
            security_groups: vec!["sg-0123456789abcdef0", "sg-0a1b2c3e"],
            tags: vec![("env", "test"), ("owner", "team-a"), ("tier", "gold")],
            ..ConnectionRequest::to(&target_one)
        },
        ConnectionRequest {
            authentication: "TLS",
            client_subnets: vec!["subnet-0a1b2c3e", "subnet-0a1b2c3d", "subnet-0a1b2c3f"],
            tags: vec![("only", "one")],
            ..ConnectionRequest::to(&target_two)
        },
    ];
    // Every published authentication type is covered, including any this file
    // did not spell out above.
    for authentication in VPC_CONNECTION_AUTHENTICATION_TYPES {
        if !rows.iter().any(|row| row.authentication == authentication) {
            rows.push(ConnectionRequest {
                authentication,
                ..ConnectionRequest::to(&target_one)
            });
        }
    }

    for request in &rows {
        let created = emulator.create_vpc_connection(request).await;
        assert_eq!(
            created.status, 200,
            "case {request:?}: expected CreateVpcConnection to return 200, got {} {}",
            created.status, created.body
        );
        let arn = created.body["vpcConnectionArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // The ARN is the service's own: this region, this account, the target
        // cluster's name, and an id of the connection's own.
        let cluster_name = request
            .target_cluster_arn
            .split('/')
            .nth(1)
            .expect("a cluster ARN carries its name");
        let prefix =
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:vpc-connection/{ACCOUNT}/{cluster_name}/");
        assert!(
            arn.starts_with(&prefix),
            "case {request:?}: expected an ARN under {prefix:?}, got {arn:?}"
        );
        assert!(
            arn[prefix.len()..].len() > 36,
            "case {request:?}: expected a uuid-shaped connection id, got {arn:?}"
        );
        // The create echoes the submitted configuration back, under its own
        // member names.
        assert_eq!(
            created.body["authentication"].as_str(),
            Some(request.authentication),
            "case {request:?}: expected the create to report the authentication type, got {}",
            created.body
        );
        assert_eq!(
            created.body["vpcId"].as_str(),
            Some(request.vpc_id),
            "case {request:?}: expected the create to report the VPC id, got {}",
            created.body
        );
        assert_eq!(
            created.body["clientSubnets"],
            json!(request.client_subnets),
            "case {request:?}: expected the create to report the client subnets in order, got {}",
            created.body
        );
        assert_eq!(
            created.body["securityGroups"],
            json!(request.security_groups),
            "case {request:?}: expected the create to report the security groups in order, got {}",
            created.body
        );
        assert_eq!(
            created.body["tags"],
            request.tag_json(),
            "case {request:?}: expected the create to report the submitted tags, got {}",
            created.body
        );
        let created_at = created.body["creationTime"].clone();
        assert!(
            !created_at.is_null(),
            "case {request:?}: expected the create to report a creationTime, got {}",
            created.body
        );

        sleep_past_vpc_connection_activation();
        let described = emulator.describe_vpc_connection(&arn).await;
        assert_eq!(
            described.status, 200,
            "case {request:?}: expected DescribeVpcConnection to return 200, got {} {}",
            described.status, described.body
        );
        for member in DESCRIBE_MEMBERS {
            assert!(
                !described.at(&format!("/{member}")).is_null(),
                "case {request:?}: expected DescribeVpcConnection to report {member}, got {}",
                described.body
            );
        }
        assert_eq!(
            described.body["vpcConnectionArn"].as_str(),
            Some(arn.as_str()),
            "case {request:?}: expected the reader to name the connection, got {}",
            described.body
        );
        assert_eq!(
            described.body["targetClusterArn"].as_str(),
            Some(request.target_cluster_arn.as_str()),
            "case {request:?}: expected the reader to name the target cluster, got {}",
            described.body
        );
        assert_eq!(
            described.body["authentication"].as_str(),
            Some(request.authentication),
            "case {request:?}: expected the reader to report the authentication type, got {}",
            described.body
        );
        assert_eq!(
            described.body["vpcId"].as_str(),
            Some(request.vpc_id),
            "case {request:?}: expected the reader to report the VPC id, got {}",
            described.body
        );
        // `DescribeVpcConnection` reports the client subnets as `Subnets`; they
        // are the same stored list, in the same order.
        assert_eq!(
            described.body["subnets"],
            json!(request.client_subnets),
            "case {request:?}: expected the reader to report the subnets in order, got {}",
            described.body
        );
        assert_eq!(
            described.body["securityGroups"],
            json!(request.security_groups),
            "case {request:?}: expected the reader to report the security groups in order, got {}",
            described.body
        );
        assert_eq!(
            described.body["tags"],
            request.tag_json(),
            "case {request:?}: expected the reader to report the stored tags, got {}",
            described.body
        );
        assert_eq!(
            described.body["creationTime"], created_at,
            "case {request:?}: expected the creation instant to be stable across reads, got {}",
            described.body
        );
        assert_eq!(
            described.body["state"].as_str(),
            Some("AVAILABLE"),
            "case {request:?}: expected a settled connection to be AVAILABLE, got {}",
            described.body
        );
    }
}

/// RULE: a resource the service reports in a non-terminal state reaches its
/// terminal one because the clock passed, not because somebody read it — a
/// caller acting immediately still sees `CREATING`, and a later one sees
/// `AVAILABLE` without any further call.
#[tokio::test]
async fn a_new_connection_becomes_available_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let target = cluster(&emulator, "clock").await;
    // Rows differ in shape and size, so the transition is not keyed to one
    // shape of input.
    let rows = [
        ConnectionRequest::to(&target),
        ConnectionRequest {
            authentication: "TLS",
            client_subnets: vec!["subnet-0a1b2c3d", "subnet-0a1b2c3e", "subnet-0a1b2c3f"],
            security_groups: vec!["sg-0a1b2c3d", "sg-0a1b2c3e"],
            tags: vec![("k", "v")],
            ..ConnectionRequest::to(&target)
        },
    ];
    for request in &rows {
        let created = emulator.create_vpc_connection(request).await;
        assert_eq!(
            created.body["state"].as_str(),
            Some("CREATING"),
            "case {request:?}: expected the create to answer CREATING, got {}",
            created.body
        );
        let arn = created.body["vpcConnectionArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let immediately = emulator.describe_vpc_connection(&arn).await;
        assert_eq!(
            immediately.body["state"].as_str(),
            Some("CREATING"),
            "case {request:?}: a describe straight after the create must still see CREATING, got {}",
            immediately.body
        );

        sleep_past_vpc_connection_activation();
        let later = emulator.describe_vpc_connection(&arn).await;
        assert_eq!(
            later.body["state"].as_str(),
            Some("AVAILABLE"),
            "case {request:?}: the connection must be AVAILABLE once its span has passed, got {}",
            later.body
        );
        // Reading it again does not move it on: the clock owed one transition.
        let again = emulator.describe_vpc_connection(&arn).await;
        assert_eq!(
            again.body["state"].as_str(),
            Some("AVAILABLE"),
            "case {request:?}: a settled connection stays AVAILABLE, got {}",
            again.body
        );
    }
}

/// RULE: the readers separate an `Arn` they cannot parse as an MSK
/// VPC-connection ARN (`BadRequestException`, 400, with the service's fixed
/// bad-parameter message) from a well-formed one no live connection holds in
/// the endpoint's region (`NotFoundException`, 404).
#[tokio::test]
async fn an_unknown_or_malformed_connection_arn_is_refused() {
    let emulator = Emulator::new();
    let target = cluster(&emulator, "present").await;
    let live = emulator
        .create_vpc_connection_ok(&ConnectionRequest::to(&target))
        .await;

    // `(arn, expected status, expected error code)`. Rows: well-formed but
    // absent (two regions in the ARN), then one row per way the ARN grammar can
    // fail — not an ARN at all, truncated, wrong service, wrong resource type
    // (including this service's own cluster ARN), too few and too many resource
    // segments, an empty segment, and an empty region.
    let rows: Vec<(String, u16, &str)> = vec![
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:vpc-connection/{ACCOUNT}/present/11111111-2222-4333-8444-555555555555-9"),
            404,
            "NotFoundException",
        ),
        (
            format!("arn:aws:kafka:us-east-1:{ACCOUNT}:vpc-connection/{ACCOUNT}/present/11111111-2222-4333-8444-555555555555-9"),
            404,
            "NotFoundException",
        ),
        ("not-an-arn".to_string(), 400, "BadRequestException"),
        ("arn:aws:kafka".to_string(), 400, "BadRequestException"),
        (
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:vpc-connection/{ACCOUNT}/present/abc-1"),
            400,
            "BadRequestException",
        ),
        (target.clone(), 400, "BadRequestException"),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:vpc-connection/{ACCOUNT}/present"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:vpc-connection/{ACCOUNT}/present/abc-1/extra"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:vpc-connection/{ACCOUNT}//abc-1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka::{ACCOUNT}:vpc-connection/{ACCOUNT}/present/abc-1"),
            400,
            "BadRequestException",
        ),
    ];

    for (arn, status, code) in &rows {
        let described = emulator.describe_vpc_connection(arn).await;
        assert_eq!(
            (described.status, described.error_code()),
            (*status, *code),
            "case arn={arn:?}: expected {status} {code} from DescribeVpcConnection, got {} {}",
            described.status,
            described.body
        );
        let deleted = emulator.delete_vpc_connection(arn).await;
        assert_eq!(
            (deleted.status, deleted.error_code()),
            (*status, *code),
            "case arn={arn:?}: expected {status} {code} from DeleteVpcConnection, got {} {}",
            deleted.status,
            deleted.body
        );
        if *status == 400 {
            assert_eq!(
                described.error_message(),
                BAD_REQUEST_MESSAGE,
                "case arn={arn:?}: expected the service's bad-parameter message, got {}",
                described.body
            );
            assert_eq!(
                deleted.error_message(),
                BAD_REQUEST_MESSAGE,
                "case arn={arn:?}: expected the service's bad-parameter message, got {}",
                deleted.body
            );
        }
    }

    // A regional endpoint serves only its own region's connections, so every
    // other region answers for the live ARN as if it never existed.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        for method in ["GET", "DELETE"] {
            let reply = emulator
                .call_in(region, method, &vpc_connection_uri(&live), None)
                .await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (404, "NotFoundException"),
                "case region={region} method={method}: expected a 404 for another region's \
                 connection, got {} {}",
                reply.status,
                reply.body
            );
        }
    }

    // None of those refusals disturbed the connection that does exist.
    assert_eq!(
        emulator.describe_vpc_connection(&live).await.status,
        200,
        "a refused read or delete must leave the registry alone"
    );
}

/// RULE: a delete removes the connection, so every later reader answers as if
/// it never existed, and the delete itself reports the connection moving to
/// `DELETING`.
#[tokio::test]
async fn delete_is_observable_by_every_later_reader() {
    let emulator = Emulator::new();
    let target = cluster(&emulator, "doomed").await;
    // Rows differ in what the connection carries and in whether it has settled
    // before the delete, so the rule is not keyed to one state or shape.
    let rows = [
        (ConnectionRequest::to(&target), false),
        (
            ConnectionRequest {
                authentication: "TLS",
                vpc_id: "vpc-0123456789abcdef0",
                client_subnets: vec!["subnet-0a1b2c3d", "subnet-0a1b2c3e", "subnet-0a1b2c3f"],
                security_groups: vec!["sg-0a1b2c3d", "sg-0a1b2c3e"],
                tags: vec![("env", "test")],
                ..ConnectionRequest::to(&target)
            },
            true,
        ),
    ];
    for (request, settle_first) in &rows {
        let arn = emulator.create_vpc_connection_ok(request).await;
        if *settle_first {
            sleep_past_vpc_connection_activation();
        }
        assert_eq!(
            emulator.describe_vpc_connection(&arn).await.status,
            200,
            "case {request:?}: expected the connection to be readable before the delete"
        );

        let deleted = emulator.delete_vpc_connection(&arn).await;
        assert_eq!(
            deleted.status, 200,
            "case {request:?}: expected DeleteVpcConnection to return 200, got {} {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body["vpcConnectionArn"].as_str(),
            Some(arn.as_str()),
            "case {request:?}: expected the delete to name the connection, got {}",
            deleted.body
        );
        assert_eq!(
            deleted.body["state"].as_str(),
            Some("DELETING"),
            "case {request:?}: expected the delete to answer DELETING, got {}",
            deleted.body
        );

        let described = emulator.describe_vpc_connection(&arn).await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "NotFoundException"),
            "case {request:?}: expected a 404 after the delete, got {} {}",
            described.status,
            described.body
        );
        let again = emulator.delete_vpc_connection(&arn).await;
        assert_eq!(
            (again.status, again.error_code()),
            (404, "NotFoundException"),
            "case {request:?}: expected a second delete to answer 404, got {} {}",
            again.status,
            again.body
        );

        // And a connection created again with the same request is a NEW
        // resource, not the deleted one coming back.
        let reborn = emulator.create_vpc_connection_ok(request).await;
        assert_ne!(
            reborn, arn,
            "case {request:?}: a connection created after a delete must get its own ARN"
        );
        assert_eq!(
            emulator.describe_vpc_connection(&reborn).await.status,
            200,
            "case {request:?}: the new connection must be readable"
        );
    }
}

/// RULE: `CreateVpcConnection` refuses a request it cannot act on with
/// `BadRequestException` (400) naming the member at fault — an authentication
/// type the service does not publish, a VPC, subnet or security-group id that
/// is not an EC2 resource id, an empty id list, or a target cluster ARN this
/// endpoint cannot resolve — and registers nothing for it.
#[tokio::test]
async fn create_refuses_a_request_it_cannot_act_on() {
    let emulator = Emulator::new();
    let target = cluster(&emulator, "valid-target").await;
    let elsewhere = emulator.vpc_connection_target_in("eu-west-1", "other-region").await;
    let valid = ConnectionRequest::to(&target);

    // Rows: one per way a request can be unusable, plus the shapes that must
    // still be ACCEPTED (both published EC2 id lengths, every authentication
    // type) so the refusal is not merely a blanket no.
    let mut rows: Vec<(ConnectionRequest, bool)> = vec![
        (valid.clone(), true),
        (
            ConnectionRequest {
                vpc_id: "vpc-0123456789abcdef0",
                client_subnets: vec![
                    "subnet-0123456789abcdef0",
                    "subnet-0123456789abcdef1",
                    "subnet-0123456789abcdef2",
                ],
                security_groups: vec!["sg-0123456789abcdef0"],
                ..valid.clone()
            },
            true,
        ),
        // An authentication type outside the published set, in each way it can
        // miss: unknown, wrong case, empty.
        (
            ConnectionRequest {
                authentication: "SASL",
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                authentication: "sasl_iam",
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                authentication: "",
                ..valid.clone()
            },
            false,
        ),
        // EC2 ids: one digit short, one long, non-hex, upper case, no suffix,
        // and no prefix at all.
        (
            ConnectionRequest {
                vpc_id: "vpc-0a1b2c3",
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                vpc_id: "vpc-0a1b2c3d4",
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                vpc_id: "vpc-0a1b2c3g",
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                vpc_id: "vpc-0A1B2C3D",
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                vpc_id: "vpc-",
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                vpc_id: "0a1b2c3d",
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                client_subnets: vec!["subnet-0a1b2c3d", "sg-0a1b2c3d"],
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                client_subnets: Vec::new(),
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                security_groups: vec!["0a1b2c3d"],
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                security_groups: Vec::new(),
                ..valid.clone()
            },
            false,
        ),
        // Target cluster: a well-formed cluster ARN nothing holds, and a live
        // cluster belonging to ANOTHER region. A value that is not a cluster ARN
        // at all never reaches this validation — the authorization decision over
        // the resource the request names answers it first
        // (`vpc_connection_admission.rs`).
        (
            ConnectionRequest {
                target_cluster_arn: format!(
                    "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-9"
                ),
                ..valid.clone()
            },
            false,
        ),
        (
            ConnectionRequest {
                target_cluster_arn: elsewhere.clone(),
                ..valid.clone()
            },
            false,
        ),
    ];
    for authentication in VPC_CONNECTION_AUTHENTICATION_TYPES {
        rows.push((
            ConnectionRequest {
                authentication,
                ..valid.clone()
            },
            true,
        ));
    }

    let mut accepted = Vec::new();
    for (request, is_accepted) in &rows {
        let reply = emulator.create_vpc_connection(request).await;
        if *is_accepted {
            assert_eq!(
                reply.status, 200,
                "case {request:?}: expected the create to succeed, got {} {}",
                reply.status, reply.body
            );
            accepted.push(
                reply.body["vpcConnectionArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
            continue;
        }
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {request:?}: expected a 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
        assert!(
            reply.body["invalidParameter"].is_string(),
            "case {request:?}: expected the refusal to name the member at fault, got {}",
            reply.body
        );
    }

    // A refused create registers nothing — not even the identity it would have
    // used: the accepted connections carry the consecutive ordinals 1..=n,
    // which only holds if no refused call consumed one.
    for (index, arn) in accepted.iter().enumerate() {
        assert!(
            arn.ends_with(&format!("-{}", index + 1)),
            "case arn={arn:?}: expected the {}. accepted create to take ordinal {}, so no refused \
             create consumed one",
            index + 1,
            index + 1
        );
        assert_eq!(
            emulator.describe_vpc_connection(arn).await.status,
            200,
            "case arn={arn:?}: an accepted create must be readable back"
        );
    }
}

/// RULE: every connection gets its own identity and its own record, so two
/// connections made with identical requests are two resources and each reader
/// answers for the one its ARN names.
#[tokio::test]
async fn each_connection_gets_its_own_identity() {
    let emulator = Emulator::new();
    let first = cluster(&emulator, "twin-one").await;
    let second = cluster(&emulator, "twin-two").await;

    // Counts rather than one pair: the same request repeated against one
    // cluster, and against a second cluster, must still yield distinct ARNs.
    let mut minted: Vec<(String, String)> = Vec::new();
    for target in [&first, &second] {
        for _ in 0..3 {
            let arn = emulator
                .create_vpc_connection_ok(&ConnectionRequest::to(target))
                .await;
            minted.push((arn, target.clone()));
        }
    }

    let mut arns: Vec<&str> = minted.iter().map(|(arn, _)| arn.as_str()).collect();
    arns.sort_unstable();
    let distinct = arns.len();
    arns.dedup();
    assert_eq!(
        arns.len(),
        distinct,
        "every connection must get its own ARN, got {minted:?}"
    );

    for (arn, target) in &minted {
        let described = emulator.describe_vpc_connection(arn).await;
        assert_eq!(
            described.body["targetClusterArn"].as_str(),
            Some(target.as_str()),
            "case arn={arn:?}: expected the reader to answer for this connection's own record, got {}",
            described.body
        );
    }
}

/// RULE: the reader keeps the two id lists apart — `Subnets` reports the stored
/// client subnets and `SecurityGroups` the stored security groups, each in its
/// own submission order, however the two lists differ in length or content.
#[tokio::test]
async fn the_reader_keeps_the_subnets_and_the_security_groups_apart() {
    let emulator = Emulator::new();
    let target = cluster(&emulator, "disjoint-lists").await;
    // Rows differ in how the two lists relate: the security-group list shorter
    // than the subnet list, longer than it, and the same length with different
    // members — so a reader that swapped or merged them could not pass any row.
    // Every row names as many subnets as the target cluster, which is what a
    // connection's placement must span.
    let rows = [
        ConnectionRequest {
            client_subnets: vec!["subnet-0a1b2c3d", "subnet-0a1b2c3e", "subnet-0a1b2c3f"],
            security_groups: vec!["sg-0a1b2c30"],
            ..ConnectionRequest::to(&target)
        },
        ConnectionRequest {
            client_subnets: vec![
                "subnet-0123456789abcdef0",
                "subnet-0123456789abcdef1",
                "subnet-0123456789abcdef2",
            ],
            security_groups: vec![
                "sg-0a1b2c31",
                "sg-0a1b2c32",
                "sg-0a1b2c35",
                "sg-0a1b2c36",
            ],
            ..ConnectionRequest::to(&target)
        },
        ConnectionRequest {
            client_subnets: vec!["subnet-0a1b2c3e", "subnet-0a1b2c3d", "subnet-0a1b2c3f"],
            security_groups: vec!["sg-0a1b2c33", "sg-0a1b2c34", "sg-0a1b2c37"],
            ..ConnectionRequest::to(&target)
        },
    ];
    for request in &rows {
        let arn = emulator.create_vpc_connection_ok(request).await;
        let described = emulator.describe_vpc_connection(&arn).await;
        assert_eq!(
            described.at("/subnets"),
            &json!(request.client_subnets),
            "case {request:?}: expected `subnets` to be the submitted client subnets in order, got {}",
            described.body
        );
        assert_eq!(
            described.at("/securityGroups"),
            &json!(request.security_groups),
            "case {request:?}: expected `securityGroups` to be the submitted groups in order, got {}",
            described.body
        );
    }
}
