// SPDX-License-Identifier: CC0-1.0

//! The DB cluster lifecycle, end to end over the wire.
//!
//! The suite drives real `awsJson1_0` requests through the same [`MultiServiceRouter`]
//! the production router dispatches with, reusing the harness in
//! [`crate::db_instance_lifecycle`] (`router`, `call`, `call_in`, `ok`) so there is one
//! wire harness for this crate.
//!
//! The assertions are RULES over a table of inputs, not a replay of one recorded call:
//! `CreateDbCluster` persists a cluster plus one DB instance per node of the composition
//! the applied DB parameter group describes, derives the engine and the default port from
//! that group, and `GetDbCluster` reports the stored record — the cluster reaching
//! `AVAILABLE` only after its nodes do. The rows vary the parameter group (none, two
//! service-managed ones, a customer-created enterprise one with a different node count),
//! the engine family, the port, the storage, the enum members chosen, the list lengths
//! and the name's length.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_timestreaminfluxdb_amazontimestreaminfluxdb as influxdb;

use crate::db_instance_lifecycle::{call, call_in, ok, router_and_engine, ACCOUNT, REGION};

/// A router whose account already holds the AWS Marketplace subscription the
/// read-replicas cluster deployment type is gated behind.
///
/// Every cluster this service creates is of that deployment type (the sole member of the
/// modelled `ClusterDeploymentType` enum), so without the subscription `CreateDbCluster`
/// is refused — that rule, and the fact a refusal stores nothing, are asserted in
/// [`crate::db_cluster_marketplace_subscription`]. A buyer subscribes in AWS Marketplace,
/// out of band of every operation here, so the happy-path rows below grant the
/// subscription through the store seam before they create anything.
fn router() -> MultiServiceRouter {
    let (router, engine) = router_and_engine();
    influxdb::state::subscribe_account_to_marketplace_offering(
        &*engine,
        influxdb::state::READ_REPLICAS_OFFERING,
    )
    .expect("the shared engine holds this service's store");
    router
}

/// Members that `CreateDbClusterInput` and `GetDbClusterOutput` BOTH model and that the
/// cluster reports exactly as the request gave them, from
/// `smithy_query(query="operation", operation="CreateDbCluster,GetDbCluster")`.
///
/// `port`, `allocatedStorage`, `deploymentType` and `failoverMode` are modelled on both
/// shapes too but are not plain echoes — a cluster has one whether or not the request
/// named it — so each is asserted against the row's expectation instead.
/// `dbBackupConfigurations` is reported through the output list shape, so it is asserted
/// member by member below.
const ECHOED_CREATE_MEMBERS: &[&str] = &[
    "dbInstanceType",
    "dbParameterGroupIdentifier",
    "dbStorageType",
    "kmsKeyId",
    "logDeliveryConfiguration",
    "maintenanceSchedule",
    "name",
    "networkType",
    "publiclyAccessible",
    "vpcSecurityGroupIds",
    "vpcSubnetIds",
];

/// The roles of a node that both ingests and queries, from the modelled `InstanceMode`
/// enum.
const INGEST_QUERY_MODES: &[&str] = &["INGEST", "QUERY"];

/// The one role of a query-only node (a read replica).
const QUERY_ONLY_MODES: &[&str] = &["QUERY"];

/// The one role of a standalone compactor node.
const COMPACTOR_MODES: &[&str] = &["COMPACT"];

/// One row of the create table: the request, and what the cluster must report for what
/// the request did not set.
struct ClusterCase {
    label: String,
    body: Value,
    /// The engine the applied DB parameter group selects.
    expected_engine_type: &'static str,
    /// The port the cluster listens on: the request's, else the engine's default.
    expected_port: i64,
    /// The storage each node gets: the request's, else the floor of the modelled range.
    expected_allocated_storage: i64,
    /// `(ingestQueryInstances, queryOnlyInstances, dedicatedCompactor)` — the node
    /// composition the applied parameter group describes.
    expected_configuration: (i64, i64, bool),
}

impl ClusterCase {
    /// How many DB instances this row's cluster provisions: one per node.
    fn expected_node_count(&self) -> usize {
        let (ingest_query, query_only, compactor) = self.expected_configuration;
        (ingest_query + query_only) as usize + usize::from(compactor)
    }

    /// The roles each of this row's nodes takes, in creation order.
    fn expected_node_modes(&self) -> Vec<&'static [&'static str]> {
        let (ingest_query, query_only, compactor) = self.expected_configuration;
        let mut modes: Vec<&'static [&'static str]> = Vec::new();
        modes.extend((0..ingest_query).map(|_| INGEST_QUERY_MODES));
        modes.extend((0..query_only).map(|_| QUERY_ONLY_MODES));
        if compactor {
            modes.push(COMPACTOR_MODES);
        }
        modes
    }
}

/// The minimum a `CreateDbCluster` request must carry, per the model's `@required`
/// members (`name`, `dbInstanceType`, `vpcSubnetIds`, `vpcSecurityGroupIds`).
fn minimal_create(name: &str) -> Value {
    json!({
        "name": name,
        "dbInstanceType": "db.influx.medium",
        "vpcSubnetIds": ["subnet-aaa", "subnet-bbb"],
        "vpcSecurityGroupIds": ["sg-aaa"],
    })
}

/// The `id` of a record in a response body.
fn id_of(case: &str, body: &Value) -> String {
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string `id`, got {body}"))
        .to_string()
}

/// The `dbClusterId` a create answered with.
fn cluster_id_of(case: &str, body: &Value) -> String {
    body["dbClusterId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string `dbClusterId`, got {body}"))
        .to_string()
}

/// `^[a-zA-Z0-9]+$`, length 3..64 — the modelled `DbClusterId` / `DbInstanceId`
/// constraint.
fn is_modelled_id(id: &str) -> bool {
    (3..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `^[a-zA-Z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$`, length 3..40 — the modelled
/// `DbInstanceName` constraint, which a node's service-generated name also satisfies.
fn is_modelled_db_instance_name(name: &str) -> bool {
    (3..=40).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.split('-').all(|segment| {
            !segment.is_empty() && segment.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

/// The string list a body member carries.
fn strings(body: &Value, member: &str) -> Vec<String> {
    body[member]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The rows this suite spans. A row that needs a customer-created DB parameter group
/// creates it on `router` first, so its minted id can go into the request.
async fn create_cases(router: &MultiServiceRouter) -> Vec<ClusterCase> {
    // A customer group that composes a bigger cluster: the three node parameters of the
    // modelled `InfluxDBv3EnterpriseParameters` are the three members
    // `ClusterConfiguration` reports.
    let big = ok(
        "setup: a customer enterprise parameter group",
        call(
            router,
            "CreateDbParameterGroup",
            json!({
                "name": "ClusterNodes",
                "parameters": {
                    "InfluxDBv3Enterprise": {
                        "ingestQueryInstances": 3,
                        "queryOnlyInstances": 2,
                        "dedicatedCompactor": true
                    }
                },
            }),
        )
        .await,
    );
    let big_group = id_of("setup: a customer enterprise parameter group", &big);

    // A customer group that asks for no query-only node at all: the floor of the
    // modelled `queryOnlyInstances` range is 0, so the cluster gets none.
    let no_replica = ok(
        "setup: an enterprise group with no query-only node",
        call(
            router,
            "CreateDbParameterGroup",
            json!({
                "name": "ClusterNoReplica",
                "parameters": {
                    "InfluxDBv3Enterprise": {
                        "ingestQueryInstances": 4,
                        "queryOnlyInstances": 0,
                        "dedicatedCompactor": false
                    }
                },
            }),
        )
        .await,
    );
    let no_replica_group = id_of("setup: an enterprise group with no query-only node", &no_replica);

    // A customer group of the v2 family: it carries no node parameters, so a cluster
    // using it gets the smallest read-replicas composition.
    let v2 = ok(
        "setup: a customer v2 parameter group",
        call(
            router,
            "CreateDbParameterGroup",
            json!({ "name": "ClusterV2", "parameters": { "InfluxDBv2": { "noTasks": true } } }),
        )
        .await,
    );
    let v2_group = id_of("setup: a customer v2 parameter group", &v2);

    vec![
        ClusterCase {
            label: "only the required members: the default engine and its default port"
                .to_string(),
            body: minimal_create("minimal-cluster"),
            expected_engine_type: "INFLUXDB_V2",
            expected_port: 8086,
            expected_allocated_storage: 20,
            expected_configuration: (1, 1, false),
        },
        ClusterCase {
            label: "the service-managed v3 core group: the v3 default port".to_string(),
            body: {
                let mut body = minimal_create("core-cluster");
                body["dbParameterGroupIdentifier"] = json!("InfluxDBV3Core");
                body
            },
            expected_engine_type: "INFLUXDB_V3_CORE",
            expected_port: 8181,
            expected_allocated_storage: 20,
            expected_configuration: (1, 1, false),
        },
        ClusterCase {
            label: "the service-managed v3 enterprise group for a single node".to_string(),
            body: {
                let mut body = minimal_create("ent-cluster");
                body["dbParameterGroupIdentifier"] = json!("InfluxDBV3Enterprise1Node");
                body
            },
            expected_engine_type: "INFLUXDB_V3_ENTERPRISE",
            expected_port: 8181,
            expected_allocated_storage: 20,
            // That group configures one node taking both the ingest and the query role.
            expected_configuration: (1, 0, false),
        },
        ClusterCase {
            label: "a customer group composing three ingest-query nodes, two read \
                    replicas and a compactor"
                .to_string(),
            body: {
                let mut body = minimal_create("big-cluster");
                body["dbParameterGroupIdentifier"] = json!(big_group);
                body
            },
            expected_engine_type: "INFLUXDB_V3_ENTERPRISE",
            expected_port: 8181,
            expected_allocated_storage: 20,
            expected_configuration: (3, 2, true),
        },
        ClusterCase {
            label: "an enterprise group asking for no query-only node".to_string(),
            body: {
                let mut body = minimal_create("no-replica-cluster");
                body["dbParameterGroupIdentifier"] = json!(no_replica_group);
                body
            },
            expected_engine_type: "INFLUXDB_V3_ENTERPRISE",
            expected_port: 8181,
            expected_allocated_storage: 20,
            expected_configuration: (4, 0, false),
        },
        ClusterCase {
            label: "a customer group of the v2 family".to_string(),
            body: {
                let mut body = minimal_create("v2-cluster");
                body["dbParameterGroupIdentifier"] = json!(v2_group);
                body
            },
            expected_engine_type: "INFLUXDB_V2",
            expected_port: 8086,
            expected_allocated_storage: 20,
            expected_configuration: (1, 1, false),
        },
        ClusterCase {
            label: "every optional member set, at the published lower bounds of port and \
                    allocatedStorage"
                .to_string(),
            body: json!({
                "name": "fully-specified-1",
                "username": "myuser",
                "password": "supersecret1",
                "organization": "myorg",
                "bucket": "mybucket",
                "dbInstanceType": "db.influx.8xlarge",
                "dbStorageType": "InfluxIOIncludedT3",
                "allocatedStorage": 20,
                "port": 1024,
                "networkType": "DUAL",
                "publiclyAccessible": true,
                "deploymentType": "MULTI_NODE_READ_REPLICAS",
                "failoverMode": "NO_FAILOVER",
                "vpcSubnetIds": ["subnet-aaa", "subnet-bbb", "subnet-ccc"],
                "vpcSecurityGroupIds": ["sg-aaa", "sg-bbb"],
                "kmsKeyId": "alias/my-key",
                "logDeliveryConfiguration": {
                    "s3Configuration": { "bucketName": "log-bucket", "enabled": true }
                },
                "maintenanceSchedule": {
                    "timezone": "UTC",
                    "preferredMaintenanceWindow": "Sun:23:00-Sun:23:30"
                },
                "dbBackupConfigurations": [
                    { "type": "DAILY", "retentionDays": 7, "enabled": true }
                ],
                "tags": { "team": "data", "env": "test" },
            }),
            expected_engine_type: "INFLUXDB_V2",
            expected_port: 1024,
            expected_allocated_storage: 20,
            expected_configuration: (1, 1, false),
        },
        ClusterCase {
            label: "the published upper bounds of port, allocatedStorage and both lists"
                .to_string(),
            body: json!({
                "name": "high-bounds",
                "dbInstanceType": "db.influx.24xlarge",
                "allocatedStorage": 15360,
                "port": 65535,
                "vpcSubnetIds": [
                    "subnet-a1", "subnet-a2", "subnet-a3",
                    "subnet-a4", "subnet-a5", "subnet-a6"
                ],
                "vpcSecurityGroupIds": ["sg-b1", "sg-b2", "sg-b3", "sg-b4", "sg-b5"],
            }),
            expected_engine_type: "INFLUXDB_V2",
            expected_port: 65535,
            expected_allocated_storage: 15360,
            expected_configuration: (1, 1, false),
        },
        ClusterCase {
            label: "the shortest name the model allows".to_string(),
            body: minimal_create("abc"),
            expected_engine_type: "INFLUXDB_V2",
            expected_port: 8086,
            expected_allocated_storage: 20,
            expected_configuration: (1, 1, false),
        },
        ClusterCase {
            // DbClusterName length 3..40, so the node names derived from it are trimmed
            // back into the DbInstanceName length.
            label: "the longest name the model allows".to_string(),
            body: minimal_create("a23456789-123456789-123456789-123456789"),
            expected_engine_type: "INFLUXDB_V2",
            expected_port: 8086,
            expected_allocated_storage: 20,
            expected_configuration: (1, 1, false),
        },
    ]
}

#[tokio::test]
async fn a_created_cluster_reads_back_from_state_member_for_member() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for case in create_cases(&router).await {
        let label = case.label.as_str();
        let created = ok(label, call(&router, "CreateDbCluster", case.body.clone()).await);

        let id = cluster_id_of(label, &created);
        assert!(
            is_modelled_id(&id),
            "case {label:?}: expected a DbClusterId (3..64 alphanumerics), got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {label:?}: the service-generated id {id:?} is not unique"
        );
        minted.push(id.clone());
        assert_eq!(
            created["dbClusterStatus"].as_str(),
            Some("CREATING"),
            "case {label:?}: a new cluster is provisioning, got {:?}",
            created["dbClusterStatus"]
        );

        let fetched = ok(label, call(&router, "GetDbCluster", json!({ "dbClusterId": &id })).await);

        assert_eq!(
            fetched["id"].as_str(),
            Some(id.as_str()),
            "case {label:?}: GetDbCluster must report the id the create minted, got {fetched}"
        );
        assert_eq!(
            fetched["arn"].as_str(),
            Some(format!("arn:aws:timestream-influxdb:{REGION}:{ACCOUNT}:db-cluster/{id}").as_str()),
            "case {label:?}: unexpected arn {:?}",
            fetched["arn"]
        );
        assert_eq!(
            fetched["status"].as_str(),
            Some("CREATING"),
            "case {label:?}: the transition span must not be zero, got {:?}",
            fetched["status"]
        );

        // Every member both shapes model comes back exactly as the request gave it, and
        // a member the request omitted is reported by nobody.
        for member in ECHOED_CREATE_MEMBERS {
            assert_eq!(
                &fetched[*member], &case.body[*member],
                "case {label:?}: member {member:?} must echo the request, got {:?}",
                fetched[*member]
            );
        }

        // The members a cluster has whether or not the request named them.
        assert_eq!(
            fetched["port"].as_i64(),
            Some(case.expected_port),
            "case {label:?}: expected port {}, got {:?}",
            case.expected_port,
            fetched["port"]
        );
        assert_eq!(
            fetched["allocatedStorage"].as_i64(),
            Some(case.expected_allocated_storage),
            "case {label:?}: expected allocatedStorage {}, got {:?}",
            case.expected_allocated_storage,
            fetched["allocatedStorage"]
        );
        assert_eq!(
            fetched["engineType"].as_str(),
            Some(case.expected_engine_type),
            "case {label:?}: expected engineType {:?}, got {:?}",
            case.expected_engine_type,
            fetched["engineType"]
        );
        assert_eq!(
            fetched["deploymentType"].as_str(),
            Some("MULTI_NODE_READ_REPLICAS"),
            "case {label:?}: unexpected deploymentType {:?}",
            fetched["deploymentType"]
        );
        let expected_failover = case
            .body
            .get("failoverMode")
            .and_then(Value::as_str)
            .unwrap_or("AUTOMATIC");
        assert_eq!(
            fetched["failoverMode"].as_str(),
            Some(expected_failover),
            "case {label:?}: expected failoverMode {expected_failover:?}, got {:?}",
            fetched["failoverMode"]
        );

        // The applied group is the requested one: this emulator applies no
        // service-managed optimized default beside it.
        assert_eq!(
            fetched["effectiveDbParameterGroupIdentifier"], fetched["dbParameterGroupIdentifier"],
            "case {label:?}: the effective group must match the requested one, got {fetched}"
        );

        // The node composition the applied parameter group describes.
        let (ingest_query, query_only, compactor) = case.expected_configuration;
        assert_eq!(
            fetched["clusterConfiguration"],
            json!({
                "ingestQueryInstances": ingest_query,
                "queryOnlyInstances": query_only,
                "dedicatedCompactor": compactor,
            }),
            "case {label:?}: unexpected clusterConfiguration {:?}",
            fetched["clusterConfiguration"]
        );

        // Both endpoints carry the cluster's name and its id, and they are distinct: one
        // serves writes and reads, the other read-only traffic.
        let name = case.body["name"].as_str().expect("the row names the cluster");
        let endpoint = fetched["endpoint"].as_str().unwrap_or_default();
        let reader = fetched["readerEndpoint"].as_str().unwrap_or_default();
        for (member, value) in [("endpoint", endpoint), ("readerEndpoint", reader)] {
            assert!(
                value.starts_with(&format!("{name}-{id}"))
                    && value.ends_with(&format!(".timestream-influxdb.{REGION}.on.aws")),
                "case {label:?}: unexpected {member} {value:?}"
            );
        }
        assert_ne!(
            endpoint, reader,
            "case {label:?}: the reader endpoint must differ from the writer endpoint"
        );
        assert!(
            fetched["influxAuthParametersSecretArn"]
                .as_str()
                .unwrap_or_default()
                .starts_with(&format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:")),
            "case {label:?}: unexpected influxAuthParametersSecretArn {:?}",
            fetched["influxAuthParametersSecretArn"]
        );

        // The backup configurations come back through the output list shape, which adds
        // `nextAutomatedBackupTime` — absent, because nothing scheduled a backup.
        let requested_backups = case.body["dbBackupConfigurations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let reported_backups = fetched["dbBackupConfigurations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported_backups.len(),
            requested_backups.len(),
            "case {label:?}: expected {} backup configurations, got {:?}",
            requested_backups.len(),
            fetched["dbBackupConfigurations"]
        );
        for (requested, reported) in requested_backups.iter().zip(&reported_backups) {
            for member in ["type", "retentionDays", "enabled"] {
                assert_eq!(
                    &reported[member], &requested[member],
                    "case {label:?}: backup configuration member {member:?} drifted, \
                     got {reported}"
                );
            }
            assert!(
                reported["nextAutomatedBackupTime"].is_null(),
                "case {label:?}: nothing scheduled a backup, got {reported}"
            );
        }

        // Neither the credentials nor the tags are members of any cluster output shape.
        for member in ["password", "username", "organization", "bucket", "tags"] {
            assert!(
                fetched[member].is_null(),
                "case {label:?}: no cluster output shape models {member:?}, got {:?}",
                fetched[member]
            );
        }

        // A read changes nothing a later caller sees.
        let again = ok(label, call(&router, "GetDbCluster", json!({ "dbClusterId": &id })).await);
        assert_eq!(
            again, fetched,
            "case {label:?}: a second read must report the same record, got {again}"
        );
    }
}

#[tokio::test]
async fn a_cluster_provisions_one_db_instance_per_node_of_its_configuration() {
    let router = router();
    for case in create_cases(&router).await {
        let label = case.label.as_str();
        let created = ok(label, call(&router, "CreateDbCluster", case.body.clone()).await);
        let cluster_id = cluster_id_of(label, &created);
        let cluster = ok(
            label,
            call(&router, "GetDbCluster", json!({ "dbClusterId": &cluster_id })).await,
        );

        // Every DB instance the DB-instance read path returns for this cluster: a node a
        // cluster reports exists only if the store it lives in hands it back.
        let listed = ok(label, call(&router, "ListDbInstances", json!({})).await);
        let mut modes_seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for item in listed["items"].as_array().cloned().unwrap_or_default() {
            let node_id = id_of(label, &item);
            let node = ok(
                label,
                call(&router, "GetDbInstance", json!({ "identifier": &node_id })).await,
            );
            if node["dbClusterId"].as_str() != Some(cluster_id.as_str()) {
                continue;
            }
            let node_name = node["name"].as_str().unwrap_or_default();
            assert!(
                is_modelled_db_instance_name(node_name),
                "case {label:?}: the generated node name {node_name:?} is not a \
                 DbInstanceName"
            );
            // The node's name is generated from the cluster's own name and id, so no two
            // clusters can generate the same node name.
            let cluster_name = cluster["name"].as_str().unwrap_or_default();
            let (head, ordinal) = node_name
                .rsplit_once('-')
                .unwrap_or_else(|| panic!("case {label:?}: {node_name:?} carries no ordinal"));
            let prefix = head.strip_suffix(&format!("-{cluster_id}")).unwrap_or_else(|| {
                panic!("case {label:?}: {node_name:?} does not carry the cluster id")
            });
            assert!(
                cluster_name.starts_with(prefix) && ordinal.parse::<usize>().is_ok(),
                "case {label:?}: node name {node_name:?} is not derived from the cluster \
                 name {cluster_name:?}"
            );

            // A node runs what the cluster was created with.
            for member in [
                "port",
                "dbInstanceType",
                "dbStorageType",
                "allocatedStorage",
                "networkType",
                "publiclyAccessible",
                "vpcSubnetIds",
                "vpcSecurityGroupIds",
                "kmsKeyId",
            ] {
                assert_eq!(
                    &node[member], &cluster[member],
                    "case {label:?}: node {node_id:?} must inherit the cluster's \
                     {member:?}, got {:?}",
                    node[member]
                );
            }
            assert_eq!(
                node["deploymentType"].as_str(),
                Some("SINGLE_AZ"),
                "case {label:?}: a cluster node is deployed on its own, got {:?}",
                node["deploymentType"]
            );
            assert_eq!(
                node["status"].as_str(),
                Some("CREATING"),
                "case {label:?}: a freshly provisioned node is creating, got {:?}",
                node["status"]
            );
            assert_eq!(
                node["instanceMode"].as_str(),
                node["instanceModes"][0].as_str(),
                "case {label:?}: instanceMode must be the node's first role, got {node}"
            );
            modes_seen.insert(node_name.to_string(), strings(&node, "instanceModes"));
        }

        assert_eq!(
            modes_seen.len(),
            case.expected_node_count(),
            "case {label:?}: expected {} provisioned DB instances, got {:?}",
            case.expected_node_count(),
            modes_seen.keys().collect::<Vec<_>>()
        );

        // The roles across the node set are exactly the composition's: the nodes that
        // ingest and query, the read replicas, and the compactor when asked for.
        let mut expected: Vec<Vec<String>> = case
            .expected_node_modes()
            .iter()
            .map(|modes| modes.iter().map(|mode| mode.to_string()).collect())
            .collect();
        let mut actual: Vec<Vec<String>> = modes_seen.into_values().collect();
        expected.sort();
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {label:?}: the node roles must match the cluster's composition"
        );
    }
}

#[tokio::test]
async fn a_cluster_is_available_only_after_the_nodes_it_provisions_are() {
    let router = router();
    let case = "the clock settles the nodes, then the cluster";
    let created = ok(
        case,
        call(&router, "CreateDbCluster", minimal_create("settling-cluster")).await,
    );
    let id = cluster_id_of(case, &created);

    /// The status of every node of the cluster, in list order.
    async fn node_statuses(router: &MultiServiceRouter, case: &str) -> Vec<String> {
        let listed = ok(case, call(router, "ListDbInstances", json!({})).await);
        listed["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| item["status"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    let immediate = ok(
        case,
        call(&router, "GetDbCluster", json!({ "dbClusterId": &id })).await,
    );
    assert_eq!(
        immediate["status"].as_str(),
        Some("CREATING"),
        "case {case:?}: the span must not be zero, got {:?}",
        immediate["status"]
    );
    let statuses = node_statuses(&router, case).await;
    assert!(
        !statuses.is_empty() && statuses.iter().all(|status| status == "CREATING"),
        "case {case:?}: the nodes are provisioning too, got {statuses:?}"
    );

    // Poll both until each is up, recording WHEN each became available. The assertion is
    // the ordering, not a wall-clock window: a composite is ready only once what it
    // provisions is.
    let started = Instant::now();
    let deadline = Duration::from_secs(30);
    let mut nodes_up: Option<Duration> = None;
    let mut cluster_up: Option<Duration> = None;
    while cluster_up.is_none() && started.elapsed() < deadline {
        let statuses = node_statuses(&router, case).await;
        if nodes_up.is_none() && statuses.iter().all(|status| status == "AVAILABLE") {
            nodes_up = Some(started.elapsed());
        }
        let cluster = ok(
            case,
            call(&router, "GetDbCluster", json!({ "dbClusterId": &id })).await,
        );
        if cluster["status"].as_str() == Some("AVAILABLE") {
            cluster_up = Some(started.elapsed());
        }
        // Sampled finely enough that the two settles, one transition span apart, fall in
        // different iterations: what the assertion below reads is their ORDER, so the
        // sampling has to be finer than the span between them.
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    let nodes_up = nodes_up.unwrap_or_else(|| {
        panic!("case {case:?}: the nodes never settled within {deadline:?}");
    });
    let cluster_up = cluster_up.unwrap_or_else(|| {
        panic!("case {case:?}: the cluster never settled within {deadline:?}");
    });
    assert!(
        nodes_up < cluster_up,
        "case {case:?}: the cluster became available at {cluster_up:?}, not after its \
         nodes at {nodes_up:?}"
    );

    // Once up it stays up, and the nodes with it.
    let settled = ok(
        case,
        call(&router, "GetDbCluster", json!({ "dbClusterId": &id })).await,
    );
    assert_eq!(
        settled["status"].as_str(),
        Some("AVAILABLE"),
        "case {case:?}: expected AVAILABLE, got {:?}",
        settled["status"]
    );
    let statuses = node_statuses(&router, case).await;
    assert!(
        statuses.iter().all(|status| status == "AVAILABLE"),
        "case {case:?}: the nodes must stay available, got {statuses:?}"
    );
}

#[tokio::test]
async fn a_cluster_name_is_unique_per_region() {
    let router = router();
    // One short, one hyphenated, one at the modelled maximum length: the rule is about
    // the name being taken, not about its form.
    let names = [
        "abc".to_string(),
        "shared-cluster-name".to_string(),
        format!("z{}", "9".repeat(39)),
    ];
    let other_region = "eu-central-1";

    for name in names {
        let case = format!("name {name:?}");
        let first = ok(
            &case,
            call(&router, "CreateDbCluster", minimal_create(&name)).await,
        );
        let first_id = cluster_id_of(&case, &first);
        let before = ok(
            &case,
            call(&router, "GetDbCluster", json!({ "dbClusterId": &first_id })).await,
        );

        let (status, body) = call(&router, "CreateDbCluster", minimal_create(&name)).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a duplicate name must conflict, got {status} with {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .unwrap_or_default()
                .ends_with("ConflictException"),
            "case {case:?}: expected a ConflictException, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(first_id.as_str()),
            "case {case:?}: the conflict must name the existing cluster, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("DB_CLUSTER"),
            "case {case:?}: unexpected resourceType in {body}"
        );

        // The refused create stored nothing: the first record is untouched.
        let after = ok(
            &case,
            call(&router, "GetDbCluster", json!({ "dbClusterId": &first_id })).await,
        );
        assert_eq!(
            after["id"], before["id"],
            "case {case:?}: the conflicting create must not touch the stored record"
        );

        // The same name in another region is a different cluster, with that region's ARN.
        let elsewhere = ok(
            &case,
            call_in(
                &router,
                other_region,
                "CreateDbCluster",
                minimal_create(&name),
            )
            .await,
        );
        let other_id = cluster_id_of(&case, &elsewhere);
        assert_ne!(
            other_id, first_id,
            "case {case:?}: a cluster created in another region must get its own id"
        );
        let other = ok(
            &case,
            call_in(
                &router,
                other_region,
                "GetDbCluster",
                json!({ "dbClusterId": &other_id }),
            )
            .await,
        );
        assert_eq!(
            other["arn"].as_str(),
            Some(
                format!("arn:aws:timestream-influxdb:{other_region}:{ACCOUNT}:db-cluster/{other_id}")
                    .as_str()
            ),
            "case {case:?}: unexpected arn {:?}",
            other["arn"]
        );
    }
}

#[tokio::test]
async fn a_create_naming_a_db_parameter_group_nothing_resolves_is_refused() {
    let router = router();
    // `dbParameterGroupIdentifier` is the group's id, pattern `^[a-zA-Z0-9]+$`.
    let unknown = [
        ("an id nothing minted", "abc123def4".to_string()),
        ("the id at the modelled minimum length", "zzz".to_string()),
        ("the id at the modelled maximum length", "f".repeat(64)),
    ];

    for (case, identifier) in unknown {
        let mut body = minimal_create("unresolved-group");
        body["dbParameterGroupIdentifier"] = json!(&identifier);
        let (status, answer) = call(&router, "CreateDbCluster", body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {answer}"
        );
        assert!(
            answer["__type"]
                .as_str()
                .unwrap_or_default()
                .ends_with("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {answer}"
        );
        assert_eq!(
            answer["resourceId"].as_str(),
            Some(identifier.as_str()),
            "case {case:?}: the error must name the group asked for, got {answer}"
        );
        assert_eq!(
            answer["resourceType"].as_str(),
            Some("DB_PARAMETER_GROUP"),
            "case {case:?}: unexpected resourceType in {answer}"
        );
    }

    // Nothing above was stored: neither a cluster holding the name, nor a node.
    let listed = ok(
        "the refusals stored nothing",
        call(&router, "ListDbInstances", json!({})).await,
    );
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(0),
        "case \"the refusals stored nothing\": a refused create must persist no node, \
         got {listed}"
    );
    let created = ok(
        "the refusals stored nothing",
        call(
            &router,
            "CreateDbCluster",
            minimal_create("unresolved-group"),
        )
        .await,
    );
    assert!(
        !cluster_id_of("the refusals stored nothing", &created).is_empty(),
        "case \"the refusals stored nothing\": the name must still be free, got {created}"
    );
}

#[tokio::test]
async fn getting_a_cluster_id_nothing_created_is_not_found() {
    let router = router();
    let live = ok(
        "a live cluster",
        call(&router, "CreateDbCluster", minimal_create("live-cluster")).await,
    );
    let live_id = cluster_id_of("a live cluster", &live);

    let unknown = [
        ("an id nothing minted", "abc123def4".to_string()),
        ("the id at the modelled minimum length", "zzz".to_string()),
        ("the id at the modelled maximum length", "f".repeat(64)),
        (
            "the cluster's NAME instead of its id",
            "livecluster".to_string(),
        ),
    ];
    for (case, identifier) in unknown {
        let (status, body) = call(&router, "GetDbCluster", json!({ "dbClusterId": &identifier }))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("DB_CLUSTER"),
            "case {case:?}: unexpected resourceType in {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(identifier.as_str()),
            "case {case:?}: the error must name the id asked for, got {body}"
        );
    }

    // The live cluster is untouched by all of that.
    let still_there = ok(
        "the live cluster survives",
        call(&router, "GetDbCluster", json!({ "dbClusterId": &live_id })).await,
    );
    assert_eq!(
        still_there["id"].as_str(),
        Some(live_id.as_str()),
        "case \"the live cluster survives\": expected the stored record, got {still_there}"
    );
}

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_and_stores_nothing() {
    let router = router();
    // `DbClusterName` length 3..40; `port` range 1024..65535; `allocatedStorage` range
    // 20..15360; `vpcSubnetIds` length 1..6; `vpcSecurityGroupIds` length 1..5.
    let name_past_max = format!("a{}", "b".repeat(40));

    let cases: Vec<(&str, Value)> = vec![
        ("a name one shorter than the modelled minimum 3", minimal_create("ab")),
        ("a name one longer than the modelled maximum 40", minimal_create(&name_past_max)),
        ("a name that does not start with a letter", minimal_create("9cluster")),
        ("a name ending in a hyphen", minimal_create("cluster-")),
        ("port one below the modelled minimum 1024", {
            let mut body = minimal_create("bad-port-low");
            body["port"] = json!(1023);
            body
        }),
        ("port one above the modelled maximum 65535", {
            let mut body = minimal_create("bad-port-high");
            body["port"] = json!(65536);
            body
        }),
        ("allocatedStorage one below the modelled minimum 20", {
            let mut body = minimal_create("bad-storage-low");
            body["allocatedStorage"] = json!(19);
            body
        }),
        ("allocatedStorage one above the modelled maximum 15360", {
            let mut body = minimal_create("bad-storage-high");
            body["allocatedStorage"] = json!(15361);
            body
        }),
        ("a deploymentType the model does not declare", {
            let mut body = minimal_create("bad-deployment");
            body["deploymentType"] = json!("SINGLE_AZ");
            body
        }),
        ("seven subnet ids, one past the modelled maximum 6", {
            let mut body = minimal_create("bad-subnets");
            body["vpcSubnetIds"] = json!([
                "subnet-1", "subnet-2", "subnet-3", "subnet-4", "subnet-5", "subnet-6",
                "subnet-7"
            ]);
            body
        }),
        (
            "a request missing the required dbInstanceType",
            json!({
                "name": "no-instance-type",
                "vpcSubnetIds": ["subnet-aaa"],
                "vpcSecurityGroupIds": ["sg-aaa"],
            }),
        ),
    ];

    for (case, body) in cases {
        let (status, answer) = call(&router, "CreateDbCluster", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer}"
        );
    }

    // None of those refusals stored a cluster or a node.
    let listed = ok(
        "the refusals stored nothing",
        call(&router, "ListDbInstances", json!({})).await,
    );
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(0),
        "case \"the refusals stored nothing\": a refused create must persist nothing, \
         got {listed}"
    );
}

/// Members of `DbClusterSummary`, from `smithy_query(query="shape",
/// shape="DbClusterSummary")`. A listed cluster's summary must agree with its record.
const CLUSTER_SUMMARY_MEMBERS: &[&str] = &[
    "allocatedStorage",
    "arn",
    "dbInstanceType",
    "dbStorageType",
    "deploymentType",
    "endpoint",
    "engineType",
    "id",
    "name",
    "networkType",
    "port",
    "readerEndpoint",
    "status",
];

#[tokio::test]
async fn paging_yields_every_live_cluster_exactly_once() {
    // (clusters created, maxResults): `MaxResults` is modelled range 1..100.
    let cases: [(usize, Option<i64>); 5] = [
        (1, None),
        (2, Some(1)),
        (4, Some(2)),
        (3, Some(100)),
        (5, Some(3)),
    ];
    for (count, max_results) in cases {
        let case = format!("{count} clusters at maxResults {max_results:?}");
        let router = router();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            let created = ok(
                &case,
                call(
                    &router,
                    "CreateDbCluster",
                    minimal_create(&format!("page-{index}")),
                )
                .await,
            );
            expected.push(cluster_id_of(&case, &created));
        }
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= count + 1,
                "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
            );
            let mut request = json!({});
            if let Some(limit) = max_results {
                request["maxResults"] = json!(limit);
            }
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = ok(&case, call(&router, "ListDbClusters", request).await);
            let items = page["items"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected an `items` list, got {page}"));
            if let Some(limit) = max_results {
                assert!(
                    items.len() as i64 <= limit,
                    "case {case:?}: a page of {} exceeds maxResults {limit}",
                    items.len()
                );
            }
            for item in items {
                seen.push(id_of(&case, item));
            }
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            seen.len(),
            sorted.len(),
            "case {case:?}: paging repeated a cluster: {seen:?}"
        );
        assert_eq!(
            sorted, expected,
            "case {case:?}: paging must yield exactly the live clusters"
        );
    }
}

#[tokio::test]
async fn every_cluster_summary_agrees_with_the_full_record() {
    let router = router();
    let cases = create_cases(&router).await;
    for case in &cases {
        ok(
            &case.label,
            call(&router, "CreateDbCluster", case.body.clone()).await,
        );
    }

    let listed = ok(
        "summaries",
        call(&router, "ListDbClusters", json!({ "maxResults": 100 })).await,
    );
    let items = listed["items"].as_array().expect("an `items` list");
    assert_eq!(
        items.len(),
        cases.len(),
        "case \"summaries\": every created cluster must be listed, got {listed}"
    );
    for item in items {
        let id = id_of("summaries", item);
        let record = ok(
            &id,
            call(&router, "GetDbCluster", json!({ "dbClusterId": &id })).await,
        );
        for member in CLUSTER_SUMMARY_MEMBERS {
            // `status` is the one member the clock may advance between the two calls.
            if *member == "status" {
                continue;
            }
            assert_eq!(
                item.get(member),
                record.get(member),
                "case {id:?}: the summary's {member} disagrees with the record, \
                 {item} vs {record}"
            );
        }
        let status = item["status"].as_str().unwrap_or_default();
        assert!(
            influxdb::ops::types::shared_types::CLUSTER_STATUS_VALUES.contains(&status),
            "case {id:?}: {status:?} is not a member of the modelled ClusterStatus enum, \
             got {item}"
        );
    }
}

#[test]
fn every_cluster_value_this_service_reports_is_a_modelled_value() {
    use service_com_amazonaws_timestreaminfluxdb_amazontimestreaminfluxdb as influxdb;

    use influxdb::ops::types::shared_types::{
        CLUSTER_DEPLOYMENT_TYPE_VALUES, CLUSTER_STATUS_VALUES, DEPLOYMENT_TYPE_VALUES,
        ENGINE_TYPE_VALUES, FAILOVER_MODE_VALUES, INSTANCE_MODE_VALUES, RESOURCE_TYPE_VALUES,
    };
    use influxdb::state as st;

    for (catalog_name, catalog, value) in [
        (
            "ClusterStatus",
            CLUSTER_STATUS_VALUES,
            st::CLUSTER_STATUS_CREATING,
        ),
        (
            "ClusterStatus",
            CLUSTER_STATUS_VALUES,
            st::CLUSTER_STATUS_AVAILABLE,
        ),
        (
            "ClusterDeploymentType",
            CLUSTER_DEPLOYMENT_TYPE_VALUES,
            st::CLUSTER_DEPLOYMENT_TYPE_READ_REPLICAS,
        ),
        ("FailoverMode", FAILOVER_MODE_VALUES, st::FAILOVER_MODE_AUTOMATIC),
        (
            "DeploymentType",
            DEPLOYMENT_TYPE_VALUES,
            st::DEPLOYMENT_TYPE_SINGLE_AZ,
        ),
        ("ResourceType", RESOURCE_TYPE_VALUES, st::RESOURCE_TYPE_DB_CLUSTER),
        ("EngineType", ENGINE_TYPE_VALUES, st::ENGINE_TYPE_INFLUXDB_V2),
        (
            "EngineType",
            ENGINE_TYPE_VALUES,
            st::ENGINE_TYPE_INFLUXDB_V3_CORE,
        ),
        (
            "EngineType",
            ENGINE_TYPE_VALUES,
            st::ENGINE_TYPE_INFLUXDB_V3_ENTERPRISE,
        ),
        ("InstanceMode", INSTANCE_MODE_VALUES, st::INSTANCE_MODE_INGEST),
        ("InstanceMode", INSTANCE_MODE_VALUES, st::INSTANCE_MODE_QUERY),
        ("InstanceMode", INSTANCE_MODE_VALUES, st::INSTANCE_MODE_COMPACT),
    ] {
        assert!(
            catalog.contains(&value),
            "case {value:?}: not a member of the modelled {catalog_name} enum {catalog:?}"
        );
    }

    // A composite resource takes one span per dependency level: its nodes, then itself.
    assert_eq!(
        st::CLUSTER_TRANSITION_SPAN,
        2 * st::TRANSITION_SPAN,
        "case \"cluster transition span\": expected two node spans, got {:?}",
        st::CLUSTER_TRANSITION_SPAN
    );
}
