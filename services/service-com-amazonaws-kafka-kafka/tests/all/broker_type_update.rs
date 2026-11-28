// SPDX-License-Identifier: CC0-1.0

//! `UpdateBrokerType` end to end, over the same `MultiServiceRouter` the emulator
//! serves from, through the harness in `tests/all/kafka_harness.rs`.
//!
//! The rules under test, each asserted over a TABLE rather than one request:
//!
//! 1. an accepted update is applied ASYNCHRONOUSLY: it records a cluster
//!    operation, leaves the cluster `UPDATING` with that operation as its
//!    `ActiveOperationArn`, and lands the target instance type plus a new
//!    `CurrentVersion` only once the clock owes the operation its completion —
//!    every other member of the cluster untouched;
//! 2. a `TargetInstanceType` that could not be an Amazon MSK broker type is
//!    refused `BadRequestException`, and the cluster is left exactly as it was;
//! 3. the cluster the URI names must exist in the endpoint's region, the
//!    submitted `CurrentVersion` must be the one the cluster is at, and the
//!    cluster must be `ACTIVE`;
//! 4. each accepted update mints its own operation ARN and its own version.
//!
//! The rows differ along what each rule generalises over — instance-type family
//! and size, the direction of the change, the published length bounds of an
//! instance type and one past them, the kind of ARN, the kind of stale version,
//! and the non-`ACTIVE` state the cluster is in.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    INSTANCE_TYPE_MAX_LEN, INSTANCE_TYPE_MIN_LEN, OPERATION_STATE_COMPLETE,
    OPERATION_STATE_IN_PROGRESS, OPERATION_TYPE_UPDATE_BROKER_TYPE, RESOURCE_NOT_FOUND_MESSAGE,
};

use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// `PUT /v1/clusters/{ClusterArn}/nodes/type` — the route this suite drives.
async fn update_broker_type(
    emulator: &Emulator,
    cluster_arn: &str,
    current_version: &str,
    target_instance_type: &str,
) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/nodes/type", pct(cluster_arn)),
            Some(json!({
                "currentVersion": current_version,
                "targetInstanceType": target_instance_type,
            })),
        )
        .await
}

/// `DescribeClusterOperation`: `GET /v1/operations/{ClusterOperationArn}` — the
/// reader that reports the operation an update started.
async fn describe_operation(emulator: &Emulator, operation_arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/operations/{}", pct(operation_arn)), None)
        .await
}

/// A cluster created with `instance_type` and waited into `ACTIVE`, as
/// (ARN, `CurrentVersion`): the state an update is accepted from.
async fn active_cluster(
    emulator: &Emulator,
    name: &str,
    instance_type: &'static str,
) -> (String, String) {
    let request = ClusterRequest {
        instance_type,
        ..ClusterRequest::named(name)
    };
    let arn = emulator.create_ok(&request).await;
    sleep_past_activation();
    let described = emulator.describe(&arn).await;
    assert_eq!(
        described.at("/clusterInfo/state").as_str(),
        Some("ACTIVE"),
        "case {name}: expected the new cluster to settle ACTIVE, got {}",
        described.body
    );
    let version = described
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: DescribeCluster reported no currentVersion"))
        .to_string();
    (arn, version)
}

/// `arn:aws:kafka:<region>:<account>:cluster-operation/<name>/<cluster-id>/<operation-id>`:
/// a cluster operation hangs off the resource tail of the cluster it belongs to.
fn assert_operation_arn(operation_arn: &str, cluster_arn: &str, case: &str) {
    let cluster_tail = cluster_arn
        .rsplit(':')
        .next()
        .and_then(|resource| resource.strip_prefix("cluster/"))
        .unwrap_or_else(|| panic!("case {case}: {cluster_arn:?} is not a cluster ARN"));
    let prefix = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/{cluster_tail}/");
    assert!(
        operation_arn.starts_with(&prefix),
        "case {case}: expected an operation ARN starting {prefix:?}, got {operation_arn:?}"
    );
    let id = &operation_arn[prefix.len()..];
    let groups: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    assert!(
        id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
        "case {case}: expected a hexadecimal operation id, got {id:?}"
    );
}

/// The `CurrentVersion` form the service mints: `K` followed by 13 upper-case
/// alphanumerics.
fn assert_version_token(version: &str, case: &str) {
    assert!(
        version.len() == 14
            && version.starts_with('K')
            && version[1..]
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase()),
        "case {case}: expected a K-prefixed cluster version token, got {version:?}"
    );
}

/// An instance type of exactly `length` characters, in the `<prefix>.<family>.<size>`
/// form a broker type takes. Used to probe the published length bounds, where it
/// is the LENGTH under test — the realistic families are the rows of
/// [`an_update_lands_on_the_cluster_once_the_clock_owes_it`].
fn instance_type_of_length(length: usize) -> String {
    let head = "a.b.";
    assert!(
        length > head.len(),
        "a <prefix>.<family>.<size> value needs more than {} characters",
        head.len()
    );
    format!("{head}{}", "x".repeat(length - head.len()))
}

/// RULE: an accepted `UpdateBrokerType` does not rewrite the cluster where it
/// stands — it starts a cluster operation, so until the clock owes that operation
/// its completion the cluster reads `UPDATING` with the operation as its
/// `ActiveOperationArn` and its old instance type and version; once it lands the
/// cluster is `ACTIVE` again at the target instance type and a NEW version, with
/// every other member of the record unchanged, and the operation itself reports
/// the type it moved from and to.
#[tokio::test]
async fn an_update_lands_on_the_cluster_once_the_clock_owes_it() {
    // The rows differ along what the rule generalises over: the instance-type
    // family (the `kafka.m5` sizes `BrokerNodeGroupInfo$InstanceType`'s
    // documentation lists, the `kafka.m7g` family AWS added after that prose, and
    // the `express` brokers), the size, and the DIRECTION of the change (up, down
    // and to the type the cluster already runs).
    let rows: &[(&str, &'static str, &str)] = &[
        ("scale-up", "kafka.m5.large", "kafka.m5.xlarge"),
        ("scale-to-largest", "kafka.m5.large", "kafka.m5.24xlarge"),
        ("scale-down", "kafka.m5.4xlarge", "kafka.m5.large"),
        ("new-family", "kafka.m5.large", "kafka.m7g.16xlarge"),
        ("express-brokers", "kafka.m7g.large", "express.m7g.large"),
        ("same-type", "kafka.m5.xlarge", "kafka.m5.xlarge"),
    ];

    let emulator = Emulator::new();
    for (case, source, target) in rows {
        let (arn, version) = active_cluster(&emulator, case, source).await;
        let before = emulator.describe(&arn).await;
        let before = before.at("/clusterInfo").clone();

        let reply = update_broker_type(&emulator, &arn, &version, target).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected UpdateBrokerType to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.body["clusterArn"].as_str(),
            Some(arn.as_str()),
            "case {case}: UpdateBrokerType reported the wrong clusterArn, got {}",
            reply.body["clusterArn"]
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: UpdateBrokerType returned no clusterOperationArn")
            })
            .to_string();
        assert_operation_arn(&operation_arn, &arn, case);

        // Still rolling: the operation is the cluster's active one and nothing it
        // will change has changed yet.
        let rolling = emulator.describe(&arn).await;
        assert_eq!(
            rolling.at("/clusterInfo/state").as_str(),
            Some("UPDATING"),
            "case {case}: expected the cluster to read UPDATING, got {}",
            rolling.body
        );
        assert_eq!(
            rolling.at("/clusterInfo/activeOperationArn").as_str(),
            Some(operation_arn.as_str()),
            "case {case}: expected activeOperationArn {operation_arn:?}, got {}",
            rolling.at("/clusterInfo/activeOperationArn")
        );
        assert_eq!(
            rolling
                .at("/clusterInfo/brokerNodeGroupInfo/instanceType")
                .as_str(),
            Some(*source),
            "case {case}: the instance type must not change before the operation lands, got {}",
            rolling.at("/clusterInfo/brokerNodeGroupInfo/instanceType")
        );
        // The VERSION, by contrast, has already moved: the accepted update
        // generated the cluster's next version, which is what a further change
        // must quote, while the configuration is still the old one.
        assert_ne!(
            rolling.at("/clusterInfo/currentVersion").as_str(),
            Some(version.as_str()),
            "case {case}: an accepted update generates the cluster's next version, got {}",
            rolling.at("/clusterInfo/currentVersion")
        );

        // The operation record reports what it is changing, and that it is still
        // running.
        let operation = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            operation.status, 200,
            "case {case}: expected DescribeClusterOperation to return 200, got {} {}",
            operation.status, operation.body
        );
        assert_eq!(
            operation.at("/clusterOperationInfo/operationType").as_str(),
            Some(OPERATION_TYPE_UPDATE_BROKER_TYPE),
            "case {case}: expected the broker-type operation type, got {}",
            operation.at("/clusterOperationInfo/operationType")
        );
        assert_eq!(
            operation
                .at("/clusterOperationInfo/operationState")
                .as_str(),
            Some(OPERATION_STATE_IN_PROGRESS),
            "case {case}: expected an in-progress operation, got {}",
            operation.at("/clusterOperationInfo/operationState")
        );
        assert_eq!(
            operation.at("/clusterOperationInfo/clusterArn").as_str(),
            Some(arn.as_str()),
            "case {case}: the operation must name its cluster, got {}",
            operation.at("/clusterOperationInfo/clusterArn")
        );
        assert_eq!(
            operation
                .at("/clusterOperationInfo/sourceClusterInfo/instanceType")
                .as_str(),
            Some(*source),
            "case {case}: expected the source instance type, got {}",
            operation.at("/clusterOperationInfo/sourceClusterInfo/instanceType")
        );
        assert_eq!(
            operation
                .at("/clusterOperationInfo/targetClusterInfo/instanceType")
                .as_str(),
            Some(*target),
            "case {case}: expected the target instance type, got {}",
            operation.at("/clusterOperationInfo/targetClusterInfo/instanceType")
        );

        sleep_past_update();
        let after = emulator.describe(&arn).await;
        assert_eq!(
            after.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {case}: expected the cluster to settle ACTIVE again, got {}",
            after.body
        );
        assert_eq!(
            after.at("/clusterInfo/activeOperationArn"),
            &Value::Null,
            "case {case}: a landed operation is no longer active, got {}",
            after.at("/clusterInfo/activeOperationArn")
        );
        assert_eq!(
            after
                .at("/clusterInfo/brokerNodeGroupInfo/instanceType")
                .as_str(),
            Some(*target),
            "case {case}: expected the target instance type, got {}",
            after.at("/clusterInfo/brokerNodeGroupInfo/instanceType")
        );
        let new_version = after
            .at("/clusterInfo/currentVersion")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: DescribeCluster reported no currentVersion"))
            .to_string();
        assert_version_token(&new_version, case);
        assert_ne!(
            new_version, version,
            "case {case}: a landed update must leave the cluster at a new version, got \
             {new_version:?}"
        );

        // And the operation has finished, with the end time it finished at.
        let operation = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            operation
                .at("/clusterOperationInfo/operationState")
                .as_str(),
            Some(OPERATION_STATE_COMPLETE),
            "case {case}: expected a completed operation, got {}",
            operation.at("/clusterOperationInfo/operationState")
        );
        assert!(
            !operation.at("/clusterOperationInfo/endTime").is_null(),
            "case {case}: a completed operation must report its endTime, got {}",
            operation.body
        );

        // Everything the operation does not change is still what it was: the
        // update is a change to ONE member, not a rewrite of the record.
        let after = after.at("/clusterInfo").clone();
        for member in [
            "clusterName",
            "clusterArn",
            "numberOfBrokerNodes",
            "creationTime",
            "encryptionInfo",
            "enhancedMonitoring",
            "storageMode",
            "zookeeperConnectString",
            "currentBrokerSoftwareInfo",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case}: {member} must survive the update unchanged, got {:?}",
                after.get(member)
            );
        }
        assert_eq!(
            after.pointer("/brokerNodeGroupInfo/clientSubnets"),
            before.pointer("/brokerNodeGroupInfo/clientSubnets"),
            "case {case}: the client subnets must survive the update unchanged, got {:?}",
            after.pointer("/brokerNodeGroupInfo/clientSubnets")
        );
    }
}

/// RULE: `TargetInstanceType` becomes the cluster's
/// `BrokerNodeGroupInfo$InstanceType`, so a value that could not be one —
/// outside the model's 5..32 `length`, or not of the `<prefix>.<family>.<size>`
/// form every MSK broker type takes — is refused `BadRequestException` naming
/// that member, and the cluster is left exactly as it was.
#[tokio::test]
async fn a_target_that_could_not_be_a_broker_type_is_refused() {
    // The bounds come from the `length` trait on
    // `BrokerNodeGroupInfo$InstanceType`/`MutableClusterInfo$InstanceType`
    // (`__stringMin5Max32`), which the service crate exports as
    // INSTANCE_TYPE_MIN_LEN/INSTANCE_TYPE_MAX_LEN: each published bound is
    // accepted and each value one past it refused.
    let shortest = instance_type_of_length(INSTANCE_TYPE_MIN_LEN);
    let longest = instance_type_of_length(INSTANCE_TYPE_MAX_LEN);
    let too_long = instance_type_of_length(INSTANCE_TYPE_MAX_LEN + 1);
    let rows: Vec<(String, bool)> = vec![
        // Accepted: a documented type, and the two published length bounds.
        ("kafka.m5.2xlarge".to_string(), true),
        (shortest, true),
        (longest, true),
        // Refused: one character past the maximum, below the minimum, and every
        // way of not being <prefix>.<family>.<size>.
        (too_long, false),
        (String::new(), false),
        ("kafk".to_string(), false),
        ("kafka".to_string(), false),
        ("kafka.m5".to_string(), false),
        ("kafka..large".to_string(), false),
        (".m5.large".to_string(), false),
        ("kafka.m5.".to_string(), false),
        ("kafka.m5.large.extra".to_string(), false),
    ];

    let emulator = Emulator::new();
    for (index, (target, accepted)) in rows.iter().enumerate() {
        let case = format!("target={target:?}");
        let source = "kafka.m5.large";
        let (arn, version) =
            active_cluster(&emulator, &format!("target-row-{index}"), source).await;

        let reply = update_broker_type(&emulator, &arn, &version, target).await;
        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case}: expected 200 for a broker type within the published bounds, got {} {}",
                reply.status, reply.body
            );
            sleep_past_update();
            let after = emulator.describe(&arn).await;
            assert_eq!(
                after
                    .at("/clusterInfo/brokerNodeGroupInfo/instanceType")
                    .as_str(),
                Some(target.as_str()),
                "case {case}: expected the accepted target to land, got {}",
                after.at("/clusterInfo/brokerNodeGroupInfo/instanceType")
            );
            continue;
        }

        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400 for a value that cannot be a broker type, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {case}: expected BadRequestException, got {}",
            reply.body
        );
        assert_eq!(
            reply.body["invalidParameter"].as_str(),
            Some("targetInstanceType"),
            "case {case}: expected the refusal to name targetInstanceType, got {}",
            reply.body
        );

        // A refused update changes nothing: no operation, no new version, no new
        // instance type.
        let after = emulator.describe(&arn).await;
        assert_eq!(
            after.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {case}: a refused update must leave the cluster ACTIVE, got {}",
            after.body
        );
        assert_eq!(
            after.at("/clusterInfo/activeOperationArn"),
            &Value::Null,
            "case {case}: a refused update must start no operation, got {}",
            after.at("/clusterInfo/activeOperationArn")
        );
        assert_eq!(
            after
                .at("/clusterInfo/brokerNodeGroupInfo/instanceType")
                .as_str(),
            Some(source),
            "case {case}: a refused update must not change the instance type, got {}",
            after.at("/clusterInfo/brokerNodeGroupInfo/instanceType")
        );
        assert_eq!(
            after.at("/clusterInfo/currentVersion").as_str(),
            Some(version.as_str()),
            "case {case}: a refused update must not change the version, got {}",
            after.at("/clusterInfo/currentVersion")
        );
    }
}

/// RULE: the cluster an update names must be one this endpoint holds — a value
/// that is not an MSK cluster ARN is `BadRequestException`, and a well-formed ARN
/// no live cluster of this region holds is `NotFoundException`.
#[tokio::test]
async fn an_update_to_a_cluster_this_endpoint_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let (live_arn, live_version) = active_cluster(&emulator, "reachable", "kafka.m5.large").await;

    // A cluster that was deleted, and one that lives in another region, are both
    // ARNs this endpoint cannot resolve — exactly like one that never existed.
    let deleted_arn = emulator
        .create_ok(&ClusterRequest::named("deleted-cluster"))
        .await;
    // A cluster is deletable once it has finished provisioning, so it is waited
    // out of `CREATING` before the delete that makes its ARN unresolvable.
    sleep_past_activation();
    let deleted = emulator
        .call("DELETE", &format!("/v1/clusters/{}", pct(&deleted_arn)), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteCluster to return 200, got {} {}",
        deleted.status, deleted.body
    );
    let other_region_arn = emulator
        .create_ok_in("eu-central-1", &ClusterRequest::named("elsewhere"))
        .await;

    // The rows differ along the KIND of ARN: not an ARN at all, an ARN of another
    // service and the right service with the wrong resource type (each naming no
    // cluster for a permission to be scoped to, so each is this surface's parameter
    // refusal), a cluster resource missing its id (a cluster reference that
    // identifies none, which authorization refuses before anything resolves), then
    // a well-formed ARN of no cluster, a deleted cluster and a cluster of another
    // region (the resolution's own miss).
    let unknown = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/ghost/11111111-2222-4333-8444-555555555555-9"
    );
    let rows: &[(&str, &str, u16, &str)] = &[
        ("not-an-arn", "demo-cluster", 400, "BadRequestException"),
        (
            "wrong-service",
            "arn:aws:s3:::my-bucket",
            400,
            "BadRequestException",
        ),
        (
            "wrong-resource-type",
            "arn:aws:kafka:us-west-2:000000000000:configuration/demo/abc-1",
            400,
            "BadRequestException",
        ),
        (
            "no-cluster-id",
            "arn:aws:kafka:us-west-2:000000000000:cluster/demo",
            403,
            "AccessDeniedException",
        ),
        ("unknown-cluster", &unknown, 404, "NotFoundException"),
        ("deleted-cluster", &deleted_arn, 404, "NotFoundException"),
        ("another-region", &other_region_arn, 404, "NotFoundException"),
    ];

    for (case, arn, status, code) in rows {
        let reply = update_broker_type(&emulator, arn, &live_version, "kafka.m5.xlarge").await;
        assert_eq!(
            reply.status, *status,
            "case {case}: expected {status} for {arn:?}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case}: expected {code} for {arn:?}, got {}",
            reply.body
        );
        if *status == 404 {
            // A missing resource is reported with the service's own constant
            // message, the same one for every ARN it cannot resolve.
            assert_eq!(
                reply.error_message(),
                RESOURCE_NOT_FOUND_MESSAGE,
                "case {case}: expected the service's not-found message, got {}",
                reply.body
            );
        }
    }

    // And the cluster this endpoint DOES hold still updates, so the refusals
    // above are about the ARN and not about the service being unable to update.
    let reply = update_broker_type(&emulator, &live_arn, &live_version, "kafka.m5.xlarge").await;
    assert_eq!(
        reply.status, 200,
        "expected the live cluster to accept the same update, got {} {}",
        reply.status, reply.body
    );
}

/// RULE: an update applies to the cluster version the caller submits, so a
/// `CurrentVersion` other than the one the cluster is at is refused
/// `BadRequestException` — which makes a replayed update a refusal once the first
/// one has landed.
#[tokio::test]
async fn a_current_version_other_than_the_clusters_is_refused() {
    let emulator = Emulator::new();
    let (arn, version) = active_cluster(&emulator, "versioned", "kafka.m5.large").await;
    let (_, other_cluster_version) =
        active_cluster(&emulator, "another-cluster", "kafka.m5.large").await;

    // The rows differ along the KIND of wrong version: empty, a token of the
    // right shape that is not this cluster's, another cluster's actual version,
    // and arbitrary text.
    let rows: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("well-shaped-but-wrong", "K3AEGXETFSTCSX".to_string()),
        ("another-clusters", other_cluster_version),
        ("not-a-version", "2".to_string()),
    ];
    for (case, stale) in &rows {
        assert_ne!(
            stale, &version,
            "case {case}: the row must differ from the cluster's own version"
        );
        let reply = update_broker_type(&emulator, &arn, stale, "kafka.m5.xlarge").await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400 for currentVersion {stale:?}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {case}: expected BadRequestException, got {}",
            reply.body
        );
        assert_eq!(
            reply.body["invalidParameter"].as_str(),
            Some("currentVersion"),
            "case {case}: expected the refusal to name currentVersion, got {}",
            reply.body
        );
        let after = emulator.describe(&arn).await;
        assert_eq!(
            after.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {case}: a refused update must leave the cluster ACTIVE, got {}",
            after.body
        );
    }

    // The cluster's own version is accepted — and the SAME request replayed after
    // the update landed is refused, because the version it names is no longer the
    // cluster's.
    let accepted = update_broker_type(&emulator, &arn, &version, "kafka.m5.xlarge").await;
    assert_eq!(
        accepted.status, 200,
        "expected the cluster's own version to be accepted, got {} {}",
        accepted.status, accepted.body
    );
    sleep_past_update();
    let replayed = update_broker_type(&emulator, &arn, &version, "kafka.m5.xlarge").await;
    assert_eq!(
        replayed.status, 400,
        "expected a replayed update to be refused once the first landed, got {} {}",
        replayed.status, replayed.body
    );
    assert_eq!(
        replayed.error_code(),
        "BadRequestException",
        "expected BadRequestException for a replayed update, got {}",
        replayed.body
    );
}

/// RULE: a cluster reconfigures one change at a time, so an update is refused
/// `BadRequestException` unless the cluster is `ACTIVE` — whether it is still
/// provisioning or already rolling an update — and the refusal starts nothing.
#[tokio::test]
async fn an_update_is_refused_unless_the_cluster_is_active() {
    let emulator = Emulator::new();

    // CREATING: the cluster has not finished provisioning.
    let creating_arn = emulator
        .create_ok(&ClusterRequest::named("still-creating"))
        .await;
    let creating = emulator.describe(&creating_arn).await;
    let creating_version = creating
        .at("/clusterInfo/currentVersion")
        .as_str()
        .expect("DescribeCluster reported no currentVersion")
        .to_string();
    let reply =
        update_broker_type(&emulator, &creating_arn, &creating_version, "kafka.m5.xlarge").await;
    assert_eq!(
        reply.status, 400,
        "case CREATING: expected 400 while the cluster is provisioning, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "BadRequestException",
        "case CREATING: expected BadRequestException, got {}",
        reply.body
    );
    let described = emulator.describe(&creating_arn).await;
    assert_eq!(
        described.at("/clusterInfo/activeOperationArn"),
        &Value::Null,
        "case CREATING: a refused update must start no operation, got {}",
        described.at("/clusterInfo/activeOperationArn")
    );

    // UPDATING: an operation of its own is already in flight. The first update's
    // target is the one that lands; the second changes nothing.
    let (arn, version) = active_cluster(&emulator, "already-updating", "kafka.m5.large").await;
    let first = update_broker_type(&emulator, &arn, &version, "kafka.m5.xlarge").await;
    assert_eq!(
        first.status, 200,
        "case UPDATING: expected the first update to be accepted, got {} {}",
        first.status, first.body
    );
    let first_operation = first.body["clusterOperationArn"]
        .as_str()
        .expect("UpdateBrokerType returned no clusterOperationArn")
        .to_string();
    let second = update_broker_type(&emulator, &arn, &version, "kafka.m5.4xlarge").await;
    assert_eq!(
        second.status, 400,
        "case UPDATING: expected 400 while an operation is in flight, got {} {}",
        second.status, second.body
    );
    assert_eq!(
        second.error_code(),
        "BadRequestException",
        "case UPDATING: expected BadRequestException, got {}",
        second.body
    );
    let rolling = emulator.describe(&arn).await;
    assert_eq!(
        rolling.at("/clusterInfo/activeOperationArn").as_str(),
        Some(first_operation.as_str()),
        "case UPDATING: the first operation must still be the active one, got {}",
        rolling.at("/clusterInfo/activeOperationArn")
    );
    sleep_past_update();
    let after = emulator.describe(&arn).await;
    assert_eq!(
        after
            .at("/clusterInfo/brokerNodeGroupInfo/instanceType")
            .as_str(),
        Some("kafka.m5.xlarge"),
        "case UPDATING: the refused second update must not land, got {}",
        after.at("/clusterInfo/brokerNodeGroupInfo/instanceType")
    );
}

/// RULE: every accepted update is its own operation — a new operation ARN and a
/// new cluster version each time — and the cluster ends at the last target it
/// accepted.
#[tokio::test]
async fn consecutive_updates_each_mint_their_own_operation() {
    // A chain rather than a single call: the rule is about successive updates, so
    // the row count (three) is what it generalises over.
    let targets = ["kafka.m5.xlarge", "kafka.m5.4xlarge", "kafka.m7g.large"];

    let emulator = Emulator::new();
    let (arn, mut version) = active_cluster(&emulator, "chained", "kafka.m5.large").await;
    let mut operations: Vec<String> = Vec::new();
    let mut versions: Vec<String> = vec![version.clone()];

    for target in targets {
        let case = format!("target={target}");
        let reply = update_broker_type(&emulator, &arn, &version, target).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no clusterOperationArn"))
            .to_string();
        assert_operation_arn(&operation_arn, &arn, &case);
        operations.push(operation_arn);

        sleep_past_update();
        let after = emulator.describe(&arn).await;
        assert_eq!(
            after
                .at("/clusterInfo/brokerNodeGroupInfo/instanceType")
                .as_str(),
            Some(target),
            "case {case}: expected the target to land, got {}",
            after.at("/clusterInfo/brokerNodeGroupInfo/instanceType")
        );
        version = after
            .at("/clusterInfo/currentVersion")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no currentVersion"))
            .to_string();
        assert_version_token(&version, &case);
        versions.push(version.clone());
    }

    let mut distinct = operations.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        operations.len(),
        "every update must mint its own operation ARN, got {operations:?}"
    );
    let mut distinct_versions = versions.clone();
    distinct_versions.sort();
    distinct_versions.dedup();
    assert_eq!(
        distinct_versions.len(),
        versions.len(),
        "every landed update must leave the cluster at a new version, got {versions:?}"
    );

    // Every operation the chain minted is still readable, each reporting the step
    // it applied: a cluster's history keeps one record per accepted update.
    for (index, operation_arn) in operations.iter().enumerate() {
        let operation = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            operation.status, 200,
            "operation {index}: expected DescribeClusterOperation to return 200, got {} {}",
            operation.status, operation.body
        );
        assert_eq!(
            operation
                .at("/clusterOperationInfo/targetClusterInfo/instanceType")
                .as_str(),
            Some(targets[index]),
            "operation {index}: expected target {}, got {}",
            targets[index],
            operation.at("/clusterOperationInfo/targetClusterInfo/instanceType")
        );
    }
}
