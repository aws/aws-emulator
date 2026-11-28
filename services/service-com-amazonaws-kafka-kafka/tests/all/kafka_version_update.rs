// SPDX-License-Identifier: CC0-1.0

//! `UpdateClusterKafkaVersion` end to end, over the same `MultiServiceRouter` the
//! emulator serves from, through the harness in `tests/all/kafka_harness.rs`.
//!
//! The rules under test, each asserted over a TABLE rather than one request:
//!
//! 1. an accepted update is applied ASYNCHRONOUSLY: it records a cluster
//!    operation of the Apache Kafka version type, leaves the cluster `UPDATING`
//!    with that operation as its `ActiveOperationArn`, and lands the target
//!    version plus a new `CurrentVersion` only once the clock owes the operation
//!    its completion — every other member of the cluster untouched;
//! 2. `TargetKafkaVersion` must be one of the versions the cluster's version can
//!    be updated to: catalogued, still offered, and strictly newer. Anything else
//!    is refused `BadRequestException` and the cluster is left exactly as it was;
//! 3. the cluster the URI names is resolved before the body: an ARN this region
//!    holds no cluster for is a `NotFoundException` whatever the body asks for,
//!    and a malformed one a `BadRequestException`;
//! 4. the submitted `CurrentVersion` must be the one the cluster is at, the
//!    cluster must be `ACTIVE`, and a serverless cluster has no Apache Kafka
//!    version to update;
//! 5. a `ConfigurationInfo` the request names is applied with the new version and
//!    must resolve to a revision this region holds.
//!
//! The rows differ along what each rule generalises over: the spelling of a
//! version (three components, a fourth, a qualifier, a `.x` minor), the size and
//! direction of the move, the support status of source and target, the kind of
//! ARN, the kind of stale version, the cluster state and kind, and how many
//! revisions the named configuration holds.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    new_cluster_kafka_versions, BAD_REQUEST_MESSAGE, CLUSTER_ARN_MEMBER, KAFKA_VERSION_ACTIVE,
    KAFKA_VERSION_CATALOG, OPERATION_STATE_COMPLETE, OPERATION_STATE_IN_PROGRESS,
    OPERATION_TYPE_UPDATE_CLUSTER_KAFKA_VERSION, PROVISIONED_ONLY_MESSAGE,
    RESOURCE_NOT_FOUND_MESSAGE,
};

use crate::kafka_harness::{
    assert_cluster_state_refusal, pct, sleep_past_activation, sleep_past_update, ClusterRequest,
    ConfigurationRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// `PUT /v1/clusters/{ClusterArn}/version` — the route this suite drives, with
/// `body` as the row gives it.
async fn update_version(emulator: &Emulator, cluster_arn: &str, body: Value) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/version", pct(cluster_arn)),
            Some(body),
        )
        .await
}

/// The same call with the two required body members, which is what most rows
/// send.
async fn update_to(
    emulator: &Emulator,
    cluster_arn: &str,
    current_version: &str,
    target_kafka_version: &str,
) -> Reply {
    update_version(
        emulator,
        cluster_arn,
        json!({
            "currentVersion": current_version,
            "targetKafkaVersion": target_kafka_version,
        }),
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

/// `GetCompatibleKafkaVersions` for one cluster:
/// `GET /v1/compatible-kafka-versions?clusterArn=<arn>` — the reader that
/// publishes the ladder an update may climb.
async fn compatible_targets(emulator: &Emulator, cluster_arn: &str) -> Vec<String> {
    let reply = emulator
        .call(
            "GET",
            &format!(
                "/v1/compatible-kafka-versions?clusterArn={}",
                pct(cluster_arn)
            ),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected GetCompatibleKafkaVersions to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/compatibleKafkaVersions/0/targetVersions")
        .as_array()
        .unwrap_or_else(|| panic!("no targetVersions reported, got {}", reply.body))
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect()
}

/// A provisioned cluster created at `kafka_version` and waited into `ACTIVE`, as
/// (ARN, `CurrentVersion`): the state an update is accepted from.
async fn active_cluster(emulator: &Emulator, name: &str, kafka_version: &'static str) -> (String, String) {
    let request = ClusterRequest {
        kafka_version,
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

/// The Apache Kafka version a cluster reports running, as
/// `ClusterInfo$CurrentBrokerSoftwareInfo$KafkaVersion`.
async fn reported_kafka_version(emulator: &Emulator, arn: &str) -> String {
    emulator
        .describe(arn)
        .await
        .at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
        .as_str()
        .unwrap_or_default()
        .to_string()
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

/// Whether the catalog offers `target` as an update for a cluster running
/// `source`, decided here from the PUBLISHED catalog rather than from the
/// helper the handler uses: a target is offered when the service still SHIPS it to
/// a new cluster ([`new_cluster_kafka_versions`], the software an in-place update
/// installs — narrower than the versions the catalog publishes an `ACTIVE` row
/// for) and it is either a strictly newer release on the SOURCE's own upgradable
/// build line or, when the source sits at the final offered ZooKeeper-mode
/// release, the `.kraft` version of that release — the KRaft migration, which MSK
/// offers at that one release. A source the service no longer offers, and one on a
/// line MSK publishes no upgrade path off, is terminal. Source: the
/// `KAFKA_VERSION_CATALOG` rows (the Amazon MSK Developer Guide's "Supported
/// Apache Kafka versions") and the per-category offering a create refusal
/// advertises, plus the Developer Guide's rule that an MSK cluster's Apache Kafka
/// version is updated in place and cannot be downgraded.
fn catalog_offers(source: &str, target: &str) -> bool {
    let shipped = new_cluster_kafka_versions("kafka.m5.large").contains(&target);
    let source_offered = KAFKA_VERSION_CATALOG
        .iter()
        .any(|entry| entry.version == source && entry.status == KAFKA_VERSION_ACTIVE);
    if !shipped || !source_offered {
        return false;
    }
    let (source_release, source_line) = (version_key(source).0, build_line(source));
    let (target_release, target_line) = (version_key(target).0, build_line(target));
    if source_line == target_line {
        return source_line != "other" && target_release > source_release;
    }
    // The crossing to the KRaft line is offered at the newest release the catalog
    // publishes an offered ZooKeeper-mode version of, and at no other.
    let crossing = KAFKA_VERSION_CATALOG
        .iter()
        .filter(|entry| entry.status == KAFKA_VERSION_ACTIVE && build_line(entry.version).is_empty())
        .map(|entry| version_key(entry.version).0)
        .max()
        .unwrap_or_default();
    source_line.is_empty()
        && target_line == "kraft"
        && target_release == source_release
        && source_release == crossing
}

/// The build line a version is on: `"kraft"` for the KRaft metadata-mode
/// versions, `""` for the ZooKeeper-mode ones — which the `.tiered` spelling of a
/// release is also on, tiered storage being a storage option of that build — and
/// `"other"` for any other build, which carries no upgrade path.
fn build_line(version: &str) -> &str {
    match version
        .rsplit('.')
        .next()
        .filter(|last| *last != "x" && last.parse::<u32>().is_err())
        .unwrap_or_default()
    {
        "" | "tiered" => "",
        "kraft" => "kraft",
        _ => "other",
    }
}

/// A version string as comparable components: each numeric component as itself,
/// a `.x` minor as "the newest patch of this minor" (so above every numbered
/// patch), and any other component as a trailing qualifier.
fn version_key(version: &str) -> (Vec<u32>, String) {
    let mut numbers = Vec::new();
    let mut qualifier = String::new();
    for component in version.split('.') {
        if !qualifier.is_empty() {
            qualifier.push('.');
            qualifier.push_str(component);
        } else if let Ok(number) = component.parse::<u32>() {
            numbers.push(number);
        } else if component == "x" {
            numbers.push(u32::MAX);
        } else {
            qualifier.push_str(component);
        }
    }
    (numbers, qualifier)
}

/// RULE: an accepted `UpdateClusterKafkaVersion` does not rewrite the cluster
/// where it stands — it starts a cluster operation, so until the clock owes that
/// operation its completion the cluster reads `UPDATING` with the operation as
/// its `ActiveOperationArn` and still its old Apache Kafka version and
/// `CurrentVersion`; once it lands the cluster is `ACTIVE` again at the target
/// version and a NEW `CurrentVersion`, with every other member of the record
/// unchanged, and the operation itself reports the version it moved from and to.
#[tokio::test]
async fn an_update_lands_the_target_version_once_the_clock_owes_it() {
    // The rows differ along what the rule generalises over: the spelling of the
    // target (a `.x` minor, a `.kraft` version), the size of the move (one minor,
    // the whole ladder), the build line it stays on (ZooKeeper, KRaft) and the one
    // cross-line move there is (the KRaft migration). Every source is a version a
    // CREATE admits, because that is the only way a cluster reaches the registry;
    // the ladder off a version MSK no longer builds a new cluster on is published
    // by the matrix instead (`compatible_kafka_versions`).
    let rows: &[(&str, &'static str, &str)] = &[
        ("minor-bump", "3.8.x", "3.9.x"),
        ("kraft-migration", "3.9.x", "3.9.x.kraft"),
        ("kraft-next", "3.8.x.kraft", "3.9.x.kraft"),
        ("kraft-ladder", "3.8.x.kraft", "4.1.x.kraft"),
        ("kraft-step", "4.0.x.kraft", "4.1.x.kraft"),
        ("whole-ladder", "3.9.x.kraft", "4.1.x.kraft"),
    ];

    let emulator = Emulator::new();
    for (case, source, target) in rows {
        let (arn, version) = active_cluster(&emulator, case, source).await;
        let before = emulator.describe(&arn).await;
        let before = before.at("/clusterInfo").clone();

        // The reader publishes the target as an update this cluster may take.
        let offered = compatible_targets(&emulator, &arn).await;
        assert!(
            offered.iter().any(|version| version == *target),
            "case {case}: GetCompatibleKafkaVersions must offer {target:?} for a cluster running \
             {source:?}, got {offered:?}"
        );

        let reply = update_to(&emulator, &arn, &version, target).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected UpdateClusterKafkaVersion to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.body["clusterArn"].as_str(),
            Some(arn.as_str()),
            "case {case}: the update reported the wrong clusterArn, got {}",
            reply.body["clusterArn"]
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: UpdateClusterKafkaVersion returned no clusterOperationArn")
            })
            .to_string();
        assert!(
            operation_arn
                .starts_with(&format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/")),
            "case {case}: expected a cluster-operation ARN of this region, got {operation_arn:?}"
        );

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
                .at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
                .as_str(),
            Some(*source),
            "case {case}: the Kafka version must not change before the operation lands, got {}",
            rolling.at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
        );
        // The cluster VERSION, by contrast, has already moved: the accepted
        // update generated it, which is what a further change must quote.
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
            Some(OPERATION_TYPE_UPDATE_CLUSTER_KAFKA_VERSION),
            "case {case}: expected the Kafka-version operation type, got {}",
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
                .at("/clusterOperationInfo/sourceClusterInfo/kafkaVersion")
                .as_str(),
            Some(*source),
            "case {case}: expected the source Kafka version, got {}",
            operation.at("/clusterOperationInfo/sourceClusterInfo/kafkaVersion")
        );
        assert_eq!(
            operation
                .at("/clusterOperationInfo/targetClusterInfo/kafkaVersion")
                .as_str(),
            Some(*target),
            "case {case}: expected the target Kafka version, got {}",
            operation.at("/clusterOperationInfo/targetClusterInfo/kafkaVersion")
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
                .at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
                .as_str(),
            Some(*target),
            "case {case}: expected the target Kafka version to land, got {}",
            after.at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
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

        // And the ladder the reader publishes follows the version the cluster now
        // runs: the version just installed is no longer an update it can take.
        let offered = compatible_targets(&emulator, &arn).await;
        assert!(
            !offered.iter().any(|version| version == *target),
            "case {case}: {target:?} must not still be offered to a cluster running it, got \
             {offered:?}"
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
        // update reinstalls the brokers' software, it does not rewrite the record.
        let after = after.at("/clusterInfo").clone();
        for member in [
            "clusterName",
            "clusterArn",
            "numberOfBrokerNodes",
            "creationTime",
            "brokerNodeGroupInfo",
            "encryptionInfo",
            "enhancedMonitoring",
            "storageMode",
            "zookeeperConnectString",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case}: {member} must survive the update unchanged, got {:?}",
                after.get(member)
            );
        }
    }
}

/// RULE: `TargetKafkaVersion` becomes the Apache Kafka version the brokers run,
/// so it must be one the service offers as an update for the version they run
/// now — software the service still ships to a new cluster, on the source's own
/// build line and strictly newer (or the one crossing to KRaft), off a source the
/// service still offers. Every other value is refused `BadRequestException`
/// naming that member, and the cluster is left exactly as it was.
#[tokio::test]
async fn only_a_target_the_service_offers_is_accepted() {
    // The rows differ along what the rule generalises over: the source's build
    // line, and every way a target can fail to be an offered update — the version
    // the cluster already runs, an older one, a deprecated one, one the service
    // does not list at all, an abbreviated or differently cased spelling, an empty
    // value, and a version on another build line (a KRaft version of another
    // release, the KRaft version of the source's own release at a release the
    // crossing is not offered at, and the way back to ZooKeeper mode, none of
    // which MSK offers). Every source is a version a CREATE admits, since that is
    // the only way a cluster reaches the registry.
    let rows: &[(&str, &'static str, &str)] = &[
        // Offered: the next minor on the ZooKeeper ladder, the KRaft migration at
        // the release MSK offers it at, a step and a far jump along the KRaft
        // ladder, and its top.
        ("ladder-top", "3.8.x", "3.9.x"),
        ("kraft-migration", "3.9.x", "3.9.x.kraft"),
        ("kraft-next", "3.8.x.kraft", "3.9.x.kraft"),
        ("kraft-far", "3.8.x.kraft", "4.1.x.kraft"),
        // Not offered.
        // The newest `.kraft` row the catalog publishes: the service has stopped
        // shipping it, so the top of the shipped ladder is terminal.
        ("kraft-ladder-top", "4.1.x.kraft", "4.2.x.kraft"),
        ("same-version", "3.9.x", "3.9.x"),
        ("one-minor-back", "3.9.x", "3.8.x"),
        ("far-downgrade", "3.9.x", "2.8.1"),
        ("deprecated-target", "3.8.x", "3.5.1"),
        ("kraft-before-the-crossing", "3.8.x", "3.8.x.kraft"),
        ("kraft-of-another-release", "3.8.x", "3.9.x.kraft"),
        ("back-to-zookeeper", "3.9.x.kraft", "3.9.x"),
        ("uncatalogued", "3.9.x", "9.9.9"),
        ("abbreviated", "3.9.x", "3.5"),
        ("other-case", "3.9.x", "3.9.X"),
        ("empty", "3.9.x", ""),
        ("not-a-version", "3.9.x", "latest"),
    ];

    let emulator = Emulator::new();
    for (index, (case, source, target)) in rows.iter().enumerate() {
        let (arn, version) =
            active_cluster(&emulator, &format!("target-row-{index}"), source).await;
        let offered = catalog_offers(source, target);
        // The published ladder and the table agree about this row, so the rows
        // are checked against the service's own catalog rather than asserted on
        // their own.
        let published = compatible_targets(&emulator, &arn).await;
        assert_eq!(
            published.iter().any(|version| version == *target),
            offered,
            "case {case}: the published ladder {published:?} disagrees with the table about \
             {target:?}"
        );

        let reply = update_to(&emulator, &arn, &version, target).await;
        if offered {
            assert_eq!(
                reply.status, 200,
                "case {case}: expected 200 for an offered target, got {} {}",
                reply.status, reply.body
            );
            sleep_past_update();
            let landed = reported_kafka_version(&emulator, &arn).await;
            assert_eq!(
                landed, *target,
                "case {case}: expected the offered target to land, got {landed:?}"
            );
            continue;
        }

        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case}: expected 400 BadRequestException for a target the service does not \
             offer, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some("targetKafkaVersion"),
            "case {case}: the refusal must name targetKafkaVersion, got {}",
            reply.body
        );
        // A refused update changes nothing: no version moves, the cluster stays
        // serving, and no operation becomes active.
        let after = emulator.describe(&arn).await;
        assert_eq!(
            (
                after.at("/clusterInfo/state").as_str(),
                after
                    .at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
                    .as_str(),
                after.at("/clusterInfo/currentVersion").as_str(),
            ),
            (Some("ACTIVE"), Some(*source), Some(version.as_str())),
            "case {case}: a refused update must leave the cluster exactly as it was, got {}",
            after.body
        );
        assert_eq!(
            after.at("/clusterInfo/activeOperationArn"),
            &Value::Null,
            "case {case}: a refused update must start no operation, got {}",
            after.at("/clusterInfo/activeOperationArn")
        );
    }
}

/// RULE: the cluster the URI names is resolved BEFORE anything in the body is
/// judged — an ARN of the right form this endpoint's region holds no cluster for
/// is the service's one not-found refusal whatever the body asks for, and an ARN
/// that is not a cluster ARN at all is a bad request.
#[tokio::test]
async fn the_cluster_is_resolved_before_the_body() {
    let emulator = Emulator::new();
    let (present, version) = active_cluster(&emulator, "resolution", "3.8.x").await;
    // A cluster of the same shape in another region: well formed, and still not
    // this endpoint's.
    let elsewhere = emulator
        .create_ok_in("eu-west-1", &ClusterRequest::named("elsewhere"))
        .await;

    // Rows: every way the ARN can fail to name a cluster of this endpoint, each
    // tried with a body that is itself valid and with bodies whose members would
    // each be refused on a cluster that did exist.
    let absent = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
    );
    let not_found: Vec<String> = vec![absent, elsewhere];
    // A value that is no ARN at all, and one naming a resource of another KIND, are
    // no cluster reference: this surface answers each as the parameter it cannot
    // read.
    let bad_request: Vec<String> = vec![
        "not-an-arn".to_string(),
        String::new(),
        format!("arn:aws:s3:{REGION}:{ACCOUNT}:cluster/present/abc-1"),
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/present/abc-1"),
    ];
    // Cluster REFERENCES that identify no cluster: authorization refuses them
    // before the cluster is resolved and before the body is read at all.
    let unauthorized: Vec<String> = vec![
        format!("arn:aws:kafka::{ACCOUNT}:cluster/present/abc-1"),
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/present/abc-1/extra"),
    ];
    let bodies: &[(&str, Value)] = &[
        (
            "valid-body",
            json!({ "currentVersion": version, "targetKafkaVersion": "3.9.x" }),
        ),
        (
            "stale-version",
            json!({ "currentVersion": "K0000000000000", "targetKafkaVersion": "3.9.x" }),
        ),
        (
            "unoffered-target",
            json!({ "currentVersion": version, "targetKafkaVersion": "9.9.9" }),
        ),
    ];

    for (arn, expected) in not_found
        .iter()
        .map(|arn| (arn, 404))
        .chain(bad_request.iter().map(|arn| (arn, 400)))
        .chain(unauthorized.iter().map(|arn| (arn, 403)))
    {
        for (body_case, body) in bodies {
            let case = format!("arn={arn:?} body={body_case}");
            let reply = update_version(&emulator, arn, body.clone()).await;
            let code = match expected {
                404 => "NotFoundException",
                403 => "AccessDeniedException",
                _ => "BadRequestException",
            };
            assert_eq!(
                (reply.status, reply.error_code()),
                (expected, code),
                "case {case}: expected {expected} {code}, got {} {}",
                reply.status,
                reply.body
            );
            // Every refusal names the member by its WIRE name, whichever
            // condition it answers; the authorization refusal names none.
            if expected == 403 {
                continue;
            }
            let member = CLUSTER_ARN_MEMBER;
            assert_eq!(
                reply.at("/invalidParameter").as_str(),
                Some(member),
                "case {case}: the refusal must name {member}, got {}",
                reply.body
            );
            if expected == 404 {
                assert_eq!(
                    reply.error_message(),
                    RESOURCE_NOT_FOUND_MESSAGE,
                    "case {case}: expected the service's not-found wording, got {:?}",
                    reply.error_message()
                );
            } else {
                assert_eq!(
                    reply.error_message(),
                    BAD_REQUEST_MESSAGE,
                    "case {case}: expected the fixed parameter sentence, got {:?}",
                    reply.error_message()
                );
            }
        }
    }

    // The cluster that does exist is untouched by any of it.
    let after = emulator.describe(&present).await;
    assert_eq!(
        (
            after.at("/clusterInfo/state").as_str(),
            after.at("/clusterInfo/currentVersion").as_str(),
        ),
        (Some("ACTIVE"), Some(version.as_str())),
        "the cluster that exists must be untouched by refusals aimed elsewhere, got {}",
        after.body
    );
}

/// RULE: an update is accepted only from a settled provisioned cluster at the
/// version the caller quotes — a stale `CurrentVersion`, a cluster that is not
/// `ACTIVE`, and a serverless cluster (which has no Apache Kafka version of its
/// own) are each refused `BadRequestException`, and nothing is persisted.
#[tokio::test]
async fn a_stale_version_or_an_unupdatable_cluster_is_refused() {
    let emulator = Emulator::new();
    let (arn, version) = active_cluster(&emulator, "guards", "3.8.x").await;

    // Rows: every way the quoted version can fail to be the cluster's own — a
    // token no cluster is at, an empty one, the right token in the wrong case,
    // and the token with a character appended.
    let stale_rows: Vec<String> = vec![
        "K0000000000000".to_string(),
        String::new(),
        version.to_lowercase(),
        format!("{version}X"),
    ];
    for stale in &stale_rows {
        let reply = update_to(&emulator, &arn, stale, "3.9.x").await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case stale={stale:?}: expected 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some("currentVersion"),
            "case stale={stale:?}: the refusal must name currentVersion, got {}",
            reply.body
        );
    }
    let after = emulator.describe(&arn).await;
    assert_eq!(
        (
            after.at("/clusterInfo/state").as_str(),
            after.at("/clusterInfo/currentVersion").as_str(),
        ),
        (Some("ACTIVE"), Some(version.as_str())),
        "a stale quote must change nothing, got {}",
        after.body
    );

    // A cluster still being provisioned has no settled software to update.
    let creating = emulator
        .create_ok(&ClusterRequest {
            kafka_version: "3.8.x",
            ..ClusterRequest::named("still-creating")
        })
        .await;
    let creating_version = emulator
        .describe(&creating)
        .await
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let reply = update_to(&emulator, &creating, &creating_version, "3.9.x").await;
    // The refusal is about the STATE the cluster is in, which the service words in
    // one sentence naming the states a change is accepted from.
    assert_cluster_state_refusal(&reply, "creating");
    sleep_past_activation();
    assert_eq!(
        reported_kafka_version(&emulator, &creating).await,
        "3.8.x",
        "case creating: a refused update must install nothing"
    );

    // A serverless cluster manages Apache Kafka itself, so it has no version
    // this operation can move.
    let serverless = emulator
        .create_serverless_ok("serverless-version", &[vec!["subnet-a", "subnet-b"]], Some(true))
        .await;
    sleep_past_activation();
    let reply = update_to(&emulator, &serverless, "K0000000000000", "3.9.x").await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case serverless: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        PROVISIONED_ONLY_MESSAGE,
        "case serverless: expected the provisioned-only wording, got {:?}",
        reply.error_message()
    );
}

/// RULE: a `ConfigurationInfo` the request names is applied WITH the new version
/// — it becomes what every reader reports as the cluster's configuration — and it
/// must resolve to a revision this endpoint's region holds; a request that names
/// none leaves the cluster's configuration as it is.
#[tokio::test]
async fn a_named_configuration_is_applied_with_the_new_version() {
    let emulator = Emulator::new();
    // A configuration holding THREE revisions, so a row can name the first, the
    // latest and one past the end.
    let configuration = emulator
        .create_configuration_row_ok_in(
            REGION,
            &ConfigurationRequest {
                extra_revisions: vec![
                    b"num.partitions=4\n".to_vec(),
                    b"num.partitions=8\n".to_vec(),
                ],
                ..ConfigurationRequest::named("applied-with-version")
            },
        )
        .await;

    // Rows: the revisions the configuration holds (its first and its latest) are
    // applied; a revision past the end, one below the first, and an ARN that
    // resolves to no configuration are refused.
    let rows: &[(&str, &str, i64, bool)] = &[
        ("first-revision", "", 1, true),
        ("latest-revision", "", 3, true),
        ("past-the-end", "", 4, false),
        ("below-the-first", "", 0, false),
        (
            "absent-configuration",
            "arn:aws:kafka:us-west-2:000000000000:configuration/absent/11111111-2222-4333-8444-555555555555-1",
            1,
            false,
        ),
    ];

    for (index, (case, arn_override, revision, accepted)) in rows.iter().enumerate() {
        let (cluster, version) =
            active_cluster(&emulator, &format!("config-row-{index}"), "3.8.x").await;
        let configuration_arn = if arn_override.is_empty() {
            configuration.as_str()
        } else {
            arn_override
        };
        let reply = update_version(
            &emulator,
            &cluster,
            json!({
                "currentVersion": version,
                "targetKafkaVersion": "3.9.x",
                "configurationInfo": { "arn": configuration_arn, "revision": revision },
            }),
        )
        .await;

        if !accepted {
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "BadRequestException"),
                "case {case}: expected 400 BadRequestException, got {} {}",
                reply.status,
                reply.body
            );
            let after = emulator.describe(&cluster).await;
            assert_eq!(
                after.at("/clusterInfo/currentBrokerSoftwareInfo/configurationArn"),
                &Value::Null,
                "case {case}: a refused update must apply no configuration, got {}",
                after.at("/clusterInfo/currentBrokerSoftwareInfo")
            );
            assert_eq!(
                after
                    .at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
                    .as_str(),
                Some("3.8.x"),
                "case {case}: a refused update must install nothing, got {}",
                after.at("/clusterInfo/currentBrokerSoftwareInfo")
            );
            continue;
        }

        assert_eq!(
            reply.status, 200,
            "case {case}: expected 200 for a revision the configuration holds, got {} {}",
            reply.status, reply.body
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the update returned no clusterOperationArn"))
            .to_string();
        // The operation reports the configuration it is applying, and that the
        // cluster had none before it.
        let operation = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            operation
                .at("/clusterOperationInfo/targetClusterInfo/configurationInfo/arn")
                .as_str(),
            Some(configuration_arn),
            "case {case}: the operation must report the configuration it applies, got {}",
            operation.at("/clusterOperationInfo/targetClusterInfo")
        );
        assert_eq!(
            operation
                .at("/clusterOperationInfo/targetClusterInfo/configurationInfo/revision")
                .as_i64(),
            Some(*revision),
            "case {case}: the operation must report the revision it applies, got {}",
            operation.at("/clusterOperationInfo/targetClusterInfo")
        );
        assert_eq!(
            operation.at("/clusterOperationInfo/sourceClusterInfo/configurationInfo"),
            &Value::Null,
            "case {case}: the cluster held no configuration before the update, got {}",
            operation.at("/clusterOperationInfo/sourceClusterInfo")
        );

        sleep_past_update();
        let after = emulator.describe(&cluster).await;
        assert_eq!(
            (
                after
                    .at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
                    .as_str(),
                after
                    .at("/clusterInfo/currentBrokerSoftwareInfo/configurationArn")
                    .as_str(),
                after
                    .at("/clusterInfo/currentBrokerSoftwareInfo/configurationRevision")
                    .as_i64(),
            ),
            (Some("3.9.x"), Some(configuration_arn), Some(*revision)),
            "case {case}: expected the version and the configuration to land together, got {}",
            after.at("/clusterInfo/currentBrokerSoftwareInfo")
        );
    }

    // And an update that names no configuration leaves the one the cluster is
    // running alone: it is applied ON the new version, not withdrawn by it.
    let (cluster, version) = active_cluster(&emulator, "config-kept", "3.8.x").await;
    let first = update_version(
        &emulator,
        &cluster,
        json!({
            "currentVersion": version,
            "targetKafkaVersion": "3.9.x",
            "configurationInfo": { "arn": configuration.as_str(), "revision": 2 },
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case config-kept: expected the first update to be accepted, got {} {}",
        first.status, first.body
    );
    sleep_past_update();
    let version = emulator
        .describe(&cluster)
        .await
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let second = update_to(&emulator, &cluster, &version, "3.9.x.kraft").await;
    assert_eq!(
        second.status, 200,
        "case config-kept: expected the second update to be accepted, got {} {}",
        second.status, second.body
    );
    sleep_past_update();
    let after = emulator.describe(&cluster).await;
    assert_eq!(
        (
            after
                .at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
                .as_str(),
            after
                .at("/clusterInfo/currentBrokerSoftwareInfo/configurationArn")
                .as_str(),
            after
                .at("/clusterInfo/currentBrokerSoftwareInfo/configurationRevision")
                .as_i64(),
        ),
        (Some("3.9.x.kraft"), Some(configuration.as_str()), Some(2)),
        "case config-kept: an update naming no configuration must keep the stored one, got {}",
        after.at("/clusterInfo/currentBrokerSoftwareInfo")
    );
}
