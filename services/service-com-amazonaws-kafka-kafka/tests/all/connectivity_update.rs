// SPDX-License-Identifier: CC0-1.0

//! `UpdateConnectivity`, driven end to end over the same `MultiServiceRouter` the
//! emulator serves from, through the shared harness in
//! `tests/all/kafka_harness.rs`.
//!
//! RULE under test: an update of a provisioned cluster's connectivity is accepted
//! only for a cluster this endpoint's region holds that is `ACTIVE` and whose
//! stored version is the one the request names, and it is applied
//! asynchronously — the call hands back a cluster-operation ARN and the cluster
//! enters `UPDATING` with that operation as its active one, keeping its old
//! configuration and version; once the clock makes the operation due the
//! submitted settings become the cluster's own, every reader derived from them
//! changes with it, and the cluster gets a new version and returns to `ACTIVE`.
//! A refused call changes nothing.
//!
//! The rows differ along what the rule generalises over: which part of the
//! connectivity configuration the request carries (public access turned on and
//! turned off, network type, multi-VPC client authentication, several at once),
//! whether it carries ZooKeeper access instead of or beside it, and the cluster
//! it starts from — whose encryption setting and authentication mechanism decide
//! which endpoints the new configuration exposes.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{CLUSTER_ARN_MEMBER, RESOURCE_NOT_FOUND_MESSAGE};

use crate::cluster_lifecycle::BOOTSTRAP_MEMBERS;
use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// `PUT /v1/clusters/{ClusterArn}/connectivity`, signed for `region`.
async fn update_in(emulator: &Emulator, region: &str, arn: &str, body: Value) -> Reply {
    emulator
        .call_in(
            region,
            "PUT",
            &format!("/v1/clusters/{}/connectivity", pct(arn)),
            Some(body),
        )
        .await
}

/// The same call, signed for the region the clusters below live in.
async fn update(emulator: &Emulator, arn: &str, body: Value) -> Reply {
    update_in(emulator, REGION, arn, body).await
}

/// `DescribeCluster`'s `clusterInfo` for `arn` — the reader that proves what the
/// service persisted.
async fn described(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let reply = emulator.describe(arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeCluster to return 200, got {} {}",
        reply.status, reply.body
    );
    reply.at("/clusterInfo").clone()
}

/// A member of a described cluster, or `Value::Null` when it is absent.
fn member(info: &Value, pointer: &str) -> Value {
    info.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// One row of the connectivity table.
#[derive(Debug)]
struct Case {
    /// The cluster the update starts from.
    created: ClusterRequest,
    /// The `connectivityInfo` the update submits, if it carries one.
    connectivity: Option<Value>,
    /// The `zookeeperAccess.enabled` the update submits, if it carries one.
    zookeeper_access: Option<bool>,
    /// Exactly the `GetBootstrapBrokers` members the cluster must offer once the
    /// update has landed. Derived from the access paths the resulting
    /// configuration enables and the ports Amazon MSK documents for them (the
    /// `PORT_*` constants in `state.rs`); every other member of
    /// `BOOTSTRAP_MEMBERS` must be ABSENT.
    expected_bootstrap: &'static [&'static str],
}

impl Case {
    fn name(&self) -> &str {
        &self.created.name
    }

    /// The request body: the version the cluster is at, plus whichever members
    /// this row submits.
    fn body(&self, version: &str) -> Value {
        let mut body = json!({ "currentVersion": version });
        if let Some(connectivity) = &self.connectivity {
            body["connectivityInfo"] = connectivity.clone();
        }
        if let Some(enabled) = self.zookeeper_access {
            body["zookeeperAccess"] = json!({ "enabled": enabled });
        }
        body
    }
}

/// The table: one row per shape the submitted connectivity configuration can
/// take, each on a cluster whose own settings decide which endpoints that
/// configuration exposes.
fn cases() -> Vec<Case> {
    vec![
        // Public access turned ON: a service-provided-EIP cluster gains the
        // public TLS endpoint, and keeps the plaintext and TLS ones.
        Case {
            created: ClusterRequest::named("public-on"),
            connectivity: Some(json!({ "publicAccess": { "type": "SERVICE_PROVIDED_EIPS" } })),
            zookeeper_access: None,
            expected_bootstrap: &[
                "bootstrapBrokerString",
                "bootstrapBrokerStringTls",
                "bootstrapBrokerStringPublicTls",
            ],
        },
        // Public access turned OFF again: the submitted configuration REPLACES
        // the one the cluster was created with, so the public endpoint goes.
        Case {
            created: ClusterRequest {
                public_access: Some("SERVICE_PROVIDED_EIPS"),
                ..ClusterRequest::named("public-off")
            },
            connectivity: Some(json!({ "publicAccess": { "type": "DISABLED" } })),
            zookeeper_access: None,
            expected_bootstrap: &["bootstrapBrokerString", "bootstrapBrokerStringTls"],
        },
        // A different member of the same shape: the network type.
        Case {
            created: ClusterRequest::named("dual-stack"),
            connectivity: Some(json!({ "networkType": "DUAL" })),
            zookeeper_access: None,
            expected_bootstrap: &[
                "bootstrapBrokerString",
                "bootstrapBrokerStringTls",
                "bootstrapBrokerStringIpv6",
                "bootstrapBrokerStringTlsIpv6",
            ],
        },
        // Multi-VPC client authentication, on a TLS-only cluster that
        // authenticates with SASL/IAM.
        Case {
            created: ClusterRequest {
                client_broker: Some("TLS"),
                sasl_iam: Some(true),
                ..ClusterRequest::named("vpc-connectivity")
            },
            connectivity: Some(json!({
                "vpcConnectivity": {
                    "clientAuthentication": {
                        "tls": { "enabled": true },
                        "sasl": { "scram": { "enabled": true }, "iam": { "enabled": true } },
                    }
                }
            })),
            zookeeper_access: None,
            // The cluster authenticates with SASL/IAM, so the cluster-level TLS
            // path (which carries no SASL exchange) is not offered; multi-VPC
            // connectivity has its own client authentication, and the TLS one it
            // turns on authenticates a client by certificate.
            expected_bootstrap: &[
                "bootstrapBrokerStringSaslIam",
                "bootstrapBrokerStringVpcConnectivityTls",
                "bootstrapBrokerStringVpcConnectivitySaslScram",
                "bootstrapBrokerStringVpcConnectivitySaslIam",
            ],
        },
        // Every part of the shape at once, on a SASL/SCRAM cluster.
        Case {
            created: ClusterRequest {
                client_broker: Some("TLS"),
                sasl_scram: Some(true),
                ..ClusterRequest::named("public-dual-vpc")
            },
            connectivity: Some(json!({
                "publicAccess": { "type": "SERVICE_PROVIDED_EIPS" },
                "networkType": "DUAL",
                "vpcConnectivity": {
                    "clientAuthentication": { "tls": { "enabled": true } }
                }
            })),
            zookeeper_access: None,
            expected_bootstrap: &[
                "bootstrapBrokerStringSaslScram",
                "bootstrapBrokerStringPublicSaslScram",
                "bootstrapBrokerStringVpcConnectivityTls",
                "bootstrapBrokerStringSaslScramIpv6",
            ],
        },
        // ZooKeeper access ALONE: the other member the operation can carry, so
        // the broker access configuration must come out untouched.
        Case {
            created: ClusterRequest::named("zookeeper-off"),
            connectivity: None,
            zookeeper_access: Some(false),
            expected_bootstrap: &["bootstrapBrokerString", "bootstrapBrokerStringTls"],
        },
        // Both members together, on a cluster that authenticates with SASL/IAM.
        Case {
            created: ClusterRequest {
                sasl_iam: Some(true),
                ..ClusterRequest::named("connectivity-and-zookeeper")
            },
            connectivity: Some(json!({
                "publicAccess": { "type": "SERVICE_PROVIDED_EIPS" },
                "networkType": "DUAL"
            })),
            zookeeper_access: Some(true),
            expected_bootstrap: &[
                "bootstrapBrokerStringSaslIam",
                "bootstrapBrokerStringPublicSaslIam",
                "bootstrapBrokerStringSaslIamIpv6",
            ],
        },
    ]
}

/// RULE: an accepted connectivity update puts the cluster into `UPDATING` with
/// the returned operation as its active one and changes nothing else yet; once
/// the clock makes that operation due, the submitted settings are the cluster's
/// own, every reader derived from them follows, and the cluster is `ACTIVE` at a
/// new version.
#[tokio::test]
async fn a_connectivity_update_lands_once_the_clock_makes_it_due() {
    let emulator = Emulator::new();
    let cases = cases();

    // PHASE 1 — every row's starting cluster, settled to ACTIVE.
    let mut arns = Vec::new();
    for case in &cases {
        arns.push(emulator.create_ok(&case.created).await);
    }
    sleep_past_activation();

    // PHASE 2 — start each update and read the cluster back IMMEDIATELY, while
    // its operation is still in flight.
    let mut started = Vec::new();
    for (case, arn) in cases.iter().zip(&arns) {
        let before = described(&emulator, case.name(), arn).await;
        assert_eq!(
            member(&before, "/state"),
            json!("ACTIVE"),
            "case {}: a cluster must be ACTIVE before it is updated, got {}",
            case.name(),
            member(&before, "/state")
        );
        let version = member(&before, "/currentVersion")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let connectivity_before = member(&before, "/brokerNodeGroupInfo/connectivityInfo");

        let reply = update(&emulator, arn, case.body(&version)).await;
        let mid = described(&emulator, case.name(), arn).await;

        assert_eq!(
            reply.status, 200,
            "case {}: expected UpdateConnectivity to return 200, got {} {}",
            case.name(),
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.body["clusterArn"].as_str(),
            Some(arn.as_str()),
            "case {}: expected the cluster's own ARN back, got {}",
            case.name(),
            reply.body["clusterArn"]
        );
        let operation = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {}: UpdateConnectivity returned no clusterOperationArn, got {}",
                    case.name(),
                    reply.body
                )
            })
            .to_string();
        let expected_prefix =
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/{}/", case.name());
        assert!(
            operation.starts_with(&expected_prefix),
            "case {}: expected a cluster-operation ARN starting {expected_prefix:?}, got {operation:?}",
            case.name()
        );

        // In flight: the cluster is UPDATING, the operation is its active one,
        // and NOTHING it reports has changed yet.
        assert_eq!(
            member(&mid, "/state"),
            json!("UPDATING"),
            "case {}: expected the cluster to be UPDATING while its operation runs, got {}",
            case.name(),
            member(&mid, "/state")
        );
        assert_eq!(
            member(&mid, "/activeOperationArn"),
            json!(operation.clone()),
            "case {}: expected activeOperationArn {operation:?}, got {}",
            case.name(),
            member(&mid, "/activeOperationArn")
        );
        assert_ne!(
            member(&mid, "/currentVersion"),
            json!(version.clone()),
            "case {}: an accepted update generates the cluster's next version, got {}",
            case.name(),
            member(&mid, "/currentVersion")
        );
        assert_eq!(
            member(&mid, "/brokerNodeGroupInfo/connectivityInfo"),
            connectivity_before,
            "case {}: an in-flight update must not change the reported connectivity, got {}",
            case.name(),
            member(&mid, "/brokerNodeGroupInfo/connectivityInfo")
        );

        started.push((version, connectivity_before, operation));
    }

    // Each update is its own operation, so no two share an ARN.
    let mut operations: Vec<&str> = started
        .iter()
        .map(|(_, _, operation)| operation.as_str())
        .collect();
    operations.sort_unstable();
    operations.dedup();
    assert_eq!(
        operations.len(),
        started.len(),
        "every update must mint its own cluster-operation ARN, got {operations:?}"
    );

    // PHASE 3 — once the clock owes the completion, the update is the cluster's.
    sleep_past_update();
    for ((case, arn), (version, connectivity_before, operation)) in
        cases.iter().zip(&arns).zip(&started)
    {
        let after = described(&emulator, case.name(), arn).await;
        assert_eq!(
            member(&after, "/state"),
            json!("ACTIVE"),
            "case {}: expected the cluster back to ACTIVE, got {}",
            case.name(),
            member(&after, "/state")
        );
        assert_eq!(
            member(&after, "/activeOperationArn"),
            Value::Null,
            "case {}: a finished operation must not stay active, got {}",
            case.name(),
            member(&after, "/activeOperationArn")
        );
        let new_version = member(&after, "/currentVersion");
        assert_ne!(
            new_version,
            json!(version.clone()),
            "case {}: a successful update must generate a new cluster version, got {new_version}",
            case.name()
        );
        assert!(
            new_version.as_str().is_some_and(|token| {
                token.len() == 14
                    && token.starts_with('K')
                    && token[1..]
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            }),
            "case {}: expected an MSK cluster-version token, got {new_version}",
            case.name()
        );

        // The submitted connectivity configuration is now the cluster's own; a
        // row that submitted none left it exactly as it was.
        let expected_connectivity = case
            .connectivity
            .clone()
            .unwrap_or_else(|| connectivity_before.clone());
        assert_eq!(
            member(&after, "/brokerNodeGroupInfo/connectivityInfo"),
            expected_connectivity,
            "case {}: expected connectivityInfo {expected_connectivity}, got {}",
            case.name(),
            member(&after, "/brokerNodeGroupInfo/connectivityInfo")
        );

        // Every reader derived from the connectivity configuration follows it.
        let brokers = emulator.bootstrap_brokers(arn).await;
        assert_eq!(
            brokers.status, 200,
            "case {}: expected GetBootstrapBrokers to return 200, got {} {}",
            case.name(),
            brokers.status,
            brokers.body
        );
        for name in BOOTSTRAP_MEMBERS {
            let present = brokers.body.get(*name).and_then(Value::as_str);
            if case.expected_bootstrap.contains(name) {
                let endpoints = present.unwrap_or_else(|| {
                    panic!(
                        "case {}: expected {name} after the update, got {}",
                        case.name(),
                        brokers.body
                    )
                });
                // The address a bootstrap string names is the one this emulator
                // answers Apache Kafka connections on for that access method — the
                // host:port the core bound its socket at — so what is asserted here is
                // that the member is published and names an address of this emulator's
                // own.
                assert!(
                    endpoints.split(',').all(|endpoint| {
                        endpoint.rsplit_once(':').is_some_and(|(host, port)| {
                            ::service_common::data_plane_addressing::
                                names_the_loopback_interface(host)
                                && port.parse::<u16>().is_ok_and(|port| port > 0)
                        })
                    }),
                    "case {}: expected {name} to name this emulator's own brokers, got \
                     {endpoints:?}",
                    case.name()
                );
            } else {
                assert_eq!(
                    present, None,
                    "case {}: expected {name} to be absent after the update, got {present:?}",
                    case.name()
                );
            }
        }

        // The version rotated, so the version the update was issued against is
        // now stale and a repeat of the same call is refused.
        let repeat = update(&emulator, arn, case.body(version)).await;
        assert_eq!(
            (repeat.status, repeat.error_code()),
            (400, "BadRequestException"),
            "case {}: expected a stale currentVersion to be refused, got {} {}",
            case.name(),
            repeat.status,
            repeat.body
        );
        // …and the operation that already finished is not reported again.
        assert_eq!(
            member(&described(&emulator, case.name(), arn).await, "/activeOperationArn"),
            Value::Null,
            "case {}: a refused update must start no operation ({operation})",
            case.name()
        );
    }
}

/// RULE: the `NotFoundException` a request gets for a resource the service does
/// not hold carries ONE wording, the service's own, whichever operation answers
/// it and however the resource came to be missing — `UpdateConnectivity` among
/// them, which is driven over its own table in
/// `connectivity_absent_cluster_refusal.rs`.
#[tokio::test]
async fn an_absent_cluster_gets_one_wording_from_every_operation() {
    let emulator = Emulator::new();
    let arn = emulator.create_ok(&ClusterRequest::named("wording")).await;
    sleep_past_activation();

    // Rows: a well-formed ARN no cluster was ever created for, and a live
    // cluster's own ARN seen from another region's endpoint — the two ways a
    // resource can be missing from the endpoint serving the request.
    let targets: Vec<(&str, String, &str)> = vec![
        (
            "never-created",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/gone/99999999-8888-4777-8666-555555555555-7"
            ),
            REGION,
        ),
        ("another-region", arn.clone(), "ap-southeast-2"),
    ];
    // Every operation in this crate that models `NotFoundException` and reports an
    // absent cluster as one, as `(case, method, uri suffix, body)`.
    for (case, target, region) in &targets {
        let calls: Vec<(&str, &str, String, Option<Value>)> = vec![
            ("DescribeCluster", "GET", String::new(), None),
            ("DeleteCluster", "DELETE", String::new(), None),
        ];
        for (operation, method, suffix, body) in calls {
            let reply = emulator
                .call_in(
                    region,
                    method,
                    &format!("/v1/clusters/{}{suffix}", pct(target)),
                    body,
                )
                .await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (404, "NotFoundException"),
                "case {case}/{operation}: expected a 404 NotFoundException, got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.body["message"].as_str(),
                Some("The requested resource doesn\u{2019}t exist."),
                "case {case}/{operation}: expected the service's not-found wording, got {}",
                reply.body
            );
        }
    }
}

/// RULE: a connectivity update is refused — changing nothing — for a `ClusterArn`
/// this endpoint cannot parse (`BadRequestException`), for a well-formed one its
/// region holds no cluster for (`NotFoundException`: the absence reported as the
/// absence it is), for a cluster that is not `ACTIVE`, for a `CurrentVersion` that
/// is not the one the cluster is at, and for a request that names neither thing the
/// operation can change.
#[tokio::test]
async fn update_connectivity_refuses_what_it_cannot_apply() {
    let emulator = Emulator::new();
    let arn = emulator.create_ok(&ClusterRequest::named("refusals")).await;
    let connectivity = json!({ "publicAccess": { "type": "SERVICE_PROVIDED_EIPS" } });

    // A cluster still being provisioned has no settled configuration to update,
    // whatever version the caller names.
    let creating = described(&emulator, "creating", &arn).await;
    assert_eq!(
        member(&creating, "/state"),
        json!("CREATING"),
        "case creating: expected a freshly created cluster to be CREATING, got {}",
        member(&creating, "/state")
    );
    let creating_version = member(&creating, "/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let reply = update(
        &emulator,
        &arn,
        json!({ "currentVersion": creating_version, "connectivityInfo": connectivity }),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case creating: expected an update of a CREATING cluster to be refused, got {} {}",
        reply.status,
        reply.body
    );

    sleep_past_activation();
    let settled = described(&emulator, "settled", &arn).await;
    let version = member(&settled, "/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows: every way the request can fail, as `(case, ARN, signing region,
    // body, status, error code)`. A value that is no ARN at all is the parameter
    // refusal; the wrong service and the wrong resource type are no cluster
    // reference either, so they keep that same refusal; an extra segment and an
    // empty region are cluster references identifying no cluster, which
    // authorization refuses before anything resolves; then a well-formed ARN no
    // cluster holds, and this cluster's own ARN seen from another region's
    // endpoint — which this surface reports as the absence it is, the grant for
    // the action resting on the caller's own documents.
    let rows: Vec<(&str, String, &str, Value, u16, &str)> = vec![
        (
            "not-an-arn",
            "kafka-cluster".to_string(),
            REGION,
            json!({ "currentVersion": version, "connectivityInfo": connectivity }),
            400,
            "BadRequestException",
        ),
        (
            "wrong-service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:cluster/refusals/abc-1"),
            REGION,
            json!({ "currentVersion": version, "connectivityInfo": connectivity }),
            400,
            "BadRequestException",
        ),
        (
            "wrong-resource-type",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/refusals/abc"),
            REGION,
            json!({ "currentVersion": version, "connectivityInfo": connectivity }),
            400,
            "BadRequestException",
        ),
        (
            "extra-segment",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/refusals/abc/extra"),
            REGION,
            json!({ "currentVersion": version, "connectivityInfo": connectivity }),
            403,
            "AccessDeniedException",
        ),
        (
            "empty-region",
            format!("arn:aws:kafka::{ACCOUNT}:cluster/refusals/abc-1"),
            REGION,
            json!({ "currentVersion": version, "connectivityInfo": connectivity }),
            403,
            "AccessDeniedException",
        ),
        (
            "absent-cluster",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
            ),
            REGION,
            json!({ "currentVersion": version, "connectivityInfo": connectivity }),
            404,
            "NotFoundException",
        ),
        (
            "another-region",
            arn.clone(),
            "us-east-1",
            json!({ "currentVersion": version, "connectivityInfo": connectivity }),
            404,
            "NotFoundException",
        ),
        (
            "stale-version",
            arn.clone(),
            REGION,
            json!({ "currentVersion": "K3AFVY5JQAABC", "connectivityInfo": connectivity }),
            400,
            "BadRequestException",
        ),
        (
            "empty-version",
            arn.clone(),
            REGION,
            json!({ "currentVersion": "", "connectivityInfo": connectivity }),
            400,
            "BadRequestException",
        ),
        (
            "nothing-to-change",
            arn.clone(),
            REGION,
            json!({ "currentVersion": version }),
            400,
            "BadRequestException",
        ),
    ];

    for (case, target, region, body, status, code) in &rows {
        let reply = update_in(&emulator, region, target, body.clone()).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (*status, *code),
            "case {case}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
        // A refusal persists nothing: the cluster is still ACTIVE at the same
        // version, with no operation in flight and its original connectivity.
        let unchanged = described(&emulator, case, &arn).await;
        for (pointer, expected) in [
            ("/state", json!("ACTIVE")),
            ("/currentVersion", json!(version.clone())),
            ("/activeOperationArn", Value::Null),
            ("/brokerNodeGroupInfo/connectivityInfo", Value::Null),
        ] {
            assert_eq!(
                member(&unchanged, pointer),
                expected,
                "case {case}: a refused update must leave {pointer} at {expected}, got {}",
                member(&unchanged, pointer)
            );
        }
    }

    // A cluster this endpoint holds no record of is reported as the absence it is:
    // the service's one fixed not-found sentence, with the member that carried the
    // reference named by its wire name.
    let absent_arn = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
    );
    let absent = update_in(
        &emulator,
        REGION,
        &absent_arn,
        json!({ "currentVersion": version, "connectivityInfo": connectivity }),
    )
    .await;
    assert_eq!(
        (
            absent.body["message"].as_str(),
            absent.body["invalidParameter"].as_str()
        ),
        (
            Some(RESOURCE_NOT_FOUND_MESSAGE),
            Some(CLUSTER_ARN_MEMBER)
        ),
        "case absent-cluster: expected {RESOURCE_NOT_FOUND_MESSAGE:?} naming \
         {CLUSTER_ARN_MEMBER}, got {}",
        absent.body
    );

    // A cluster with an update already in flight is not ACTIVE, so a second
    // update is refused — and the first one still lands.
    let accepted = update(
        &emulator,
        &arn,
        json!({ "currentVersion": version, "connectivityInfo": connectivity }),
    )
    .await;
    let second = update(
        &emulator,
        &arn,
        json!({ "currentVersion": version, "connectivityInfo": { "networkType": "DUAL" } }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case in-flight: expected the first update to be accepted, got {} {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        (second.status, second.error_code()),
        (400, "BadRequestException"),
        "case in-flight: expected a second update to be refused, got {} {}",
        second.status,
        second.body
    );

    sleep_past_update();
    let landed = described(&emulator, "in-flight", &arn).await;
    assert_eq!(
        member(&landed, "/brokerNodeGroupInfo/connectivityInfo"),
        connectivity,
        "case in-flight: expected the accepted update to land, got {}",
        member(&landed, "/brokerNodeGroupInfo/connectivityInfo")
    );
    assert_eq!(
        member(&landed, "/state"),
        json!("ACTIVE"),
        "case in-flight: expected the cluster back to ACTIVE, got {}",
        member(&landed, "/state")
    );
}
