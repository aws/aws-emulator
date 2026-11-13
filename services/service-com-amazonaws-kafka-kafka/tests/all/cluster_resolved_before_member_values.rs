// SPDX-License-Identifier: CC0-1.0

//! The ORDER in which an MSK operation that names the cluster it acts on in its
//! URI decides its refusals, driven end to end over the same
//! `MultiServiceRouter` the emulator serves from (the shared `kafka_harness`).
//!
//! The RULE this suite asserts, over a table rather than one recorded request:
//! such an operation resolves the cluster its URI names BEFORE it judges the
//! VALUES the request's other members carry — so a request against a cluster
//! this endpoint holds nothing for is refused HTTP 404 `NotFoundException` with
//! the service's one fixed not-found sentence whatever values those members
//! carry, and the very same request against a cluster the endpoint DOES hold is
//! refused HTTP 400 `BadRequestException`, about the member at fault. Only the
//! shape of the reference itself (an ARN that is not a cluster ARN,
//! `cluster_arn_refusal.rs`) and WHICH members the request carries are decided
//! earlier, because neither names a cluster to resolve.
//!
//! The rows span what the rule generalises over: which operation answers
//! (`UpdateBrokerCount`, `UpdateBrokerType`, `UpdateBrokerStorage` and
//! `UpdateChannel`), which member carries the refused value, and what makes the
//! value refusable (above a published maximum, below a published minimum,
//! negative, the largest integer the member can carry, a string that could not be
//! a broker type, and a keyword the member does not accept) — crossed with the
//! three ways a well-formed cluster ARN resolves to nothing here: a cluster that
//! was never created, one that was deleted and one that lives in another region.
//!
//! `UpdateChannel`'s rows pin only the refusal CLASS once the cluster resolves:
//! the channel a row names is not one the live cluster holds, so the 400 it
//! answers there is about that reference rather than about the freshness value —
//! what that operation answers an out-of-range freshness on a channel it does
//! hold is `channel_destination_update.rs`'s rule.

use serde_json::{json, Value};

// The service's own fixed refusal wordings and the published bounds the rows are
// written against, so the suite and the handlers cannot drift apart on either.
use service_com_amazonaws_kafka_kafka::state::{
    ALL_BROKERS, BAD_REQUEST_MESSAGE, MAX_BROKER_NODES_PER_CLUSTER, MAX_DATA_FRESHNESS_SECONDS,
    MAX_VOLUME_SIZE_GB, MIN_DATA_FRESHNESS_SECONDS, MIN_VOLUME_SIZE_GB,
    RESOURCE_NOT_FOUND_MESSAGE,
};

// The router harness, the signing region, the account id, the URI encoder and the
// shared wait for a cluster to settle, so this one adds only its own rule.
use super::kafka_harness::{
    pct, poll_until_active, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// One row: an operation that names its cluster in the URI, called with ONE
/// member value the service refuses.
struct Row {
    /// What the row is, named in every assertion it makes.
    case: String,
    method: &'static str,
    /// The path under `/v1/clusters/{ClusterArn}` the model binds the operation
    /// to, already percent-encoded where it carries a second reference.
    suffix: String,
    body: Value,
    /// The member the refusal must report in `invalidParameter`, by its wire
    /// name, where this suite pins the wording the service answers an
    /// out-of-bound value with; `None` where another suite owns that wording.
    member: Option<&'static str>,
}

/// Every row, built against `current_version` (so a refusal is never about a
/// stale version) and `channel_arn` (the second reference `UpdateChannel` takes).
fn rows(current_version: &str, channel_arn: &str) -> Vec<Row> {
    let broker_count = |case: &str, target: i64| Row {
        case: format!("UpdateBrokerCount targetNumberOfBrokerNodes={target} ({case})"),
        method: "PUT",
        suffix: "/nodes/count".to_string(),
        body: json!({
            "currentVersion": current_version,
            "targetNumberOfBrokerNodes": target,
        }),
        // The bound is the model's own (`__integerMin1Max15` on
        // `UpdateBrokerCountRequest$TargetNumberOfBrokerNodes`), and the service
        // reports the member at fault by its `jsonName`.
        member: Some("targetNumberOfBrokerNodes"),
    };
    let broker_type = |case: &str, instance_type: &str| Row {
        case: format!("UpdateBrokerType targetInstanceType={instance_type:?} ({case})"),
        method: "PUT",
        suffix: "/nodes/type".to_string(),
        body: json!({
            "currentVersion": current_version,
            "targetInstanceType": instance_type,
        }),
        member: None,
    };
    let broker_storage = |case: &str, broker_id: &str, volume: i64| Row {
        case: format!("UpdateBrokerStorage kafkaBrokerNodeId={broker_id:?} volumeSizeGB={volume} ({case})"),
        method: "PUT",
        suffix: "/nodes/storage".to_string(),
        body: json!({
            "currentVersion": current_version,
            "targetBrokerEBSVolumeInfo": [
                { "kafkaBrokerNodeId": broker_id, "volumeSizeGB": volume },
            ],
        }),
        member: None,
    };
    let channel = |case: &str, freshness: i64| Row {
        case: format!("UpdateChannel dataFreshnessInSeconds={freshness} ({case})"),
        method: "PUT",
        suffix: format!("/channels/{}", pct(channel_arn)),
        body: json!({
            "s3DestinationUpdate": { "dataFreshnessInSeconds": freshness },
        }),
        member: None,
    };

    let max_brokers = i64::from(MAX_BROKER_NODES_PER_CLUSTER);
    vec![
        broker_count("one past the published maximum", max_brokers + 1),
        broker_count("far past the published maximum", 100),
        broker_count("the largest value the member carries", i64::from(i32::MAX)),
        broker_count("one below the published minimum", 0),
        broker_count("negative", -1),
        broker_type("not a broker type at all", "not-a-broker-type"),
        broker_type("empty", ""),
        broker_type("a family with no size", "kafka.m5"),
        broker_storage(
            "one past the published maximum volume",
            ALL_BROKERS,
            i64::from(MAX_VOLUME_SIZE_GB) + 1,
        ),
        broker_storage(
            "one below the published minimum volume",
            ALL_BROKERS,
            i64::from(MIN_VOLUME_SIZE_GB) - 1,
        ),
        broker_storage("a broker id that is not the keyword", "1", 1100),
        channel(
            "one below the published minimum",
            i64::from(MIN_DATA_FRESHNESS_SECONDS) - 1,
        ),
        channel(
            "one past the published maximum",
            i64::from(MAX_DATA_FRESHNESS_SECONDS) + 1,
        ),
    ]
}

/// Send one row against `cluster_arn`.
async fn send(emulator: &Emulator, cluster_arn: &str, row: &Row) -> Reply {
    emulator
        .call(
            row.method,
            &format!("/v1/clusters/{}{}", pct(cluster_arn), row.suffix),
            Some(row.body.clone()),
        )
        .await
}

/// A cluster created in `region` and waited into `ACTIVE`, as (ARN, version).
async fn active_cluster(emulator: &Emulator, region: &str, name: &str) -> (String, String) {
    let arn = emulator
        .create_ok_in(region, &ClusterRequest::named(name))
        .await;
    let version = poll_until_active(emulator, &arn, name).await;
    (arn, version)
}

/// RULE: the cluster the URI names is resolved FIRST, so a well-formed cluster
/// ARN this endpoint holds nothing for is refused with the service's fixed
/// not-found sentence whatever value the request's other members carry — and the
/// identical request against a cluster the endpoint DOES hold is refused as a
/// client-input fault about that member instead, leaving the cluster untouched.
#[tokio::test]
async fn a_cluster_the_endpoint_does_not_hold_is_answered_before_any_member_value() {
    let emulator = Emulator::new();

    // The cluster every row's "resolves" half runs against: live, ACTIVE and at a
    // version the rows submit, so the only thing wrong with those requests is the
    // member value under test.
    let (live_arn, live_version) = active_cluster(&emulator, REGION, "reachable").await;

    // Three ways a well-formed cluster ARN resolves to nothing at this endpoint:
    // never created, deleted, and held by another region's endpoint.
    let never_created = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/ghost/00000000-0000-4000-8000-000000000000-1"
    );
    let (deleted_arn, _) = active_cluster(&emulator, REGION, "deleted-cluster").await;
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

    // A channel reference for `UpdateChannel`'s rows: well formed, and held by no
    // cluster of this endpoint.
    let channel_arn = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:channel/ghost/00000000-0000-4000-8000-000000000000-5"
    );

    let unresolvable: [(&str, &str); 3] = [
        ("never-created", never_created.as_str()),
        ("deleted", deleted_arn.as_str()),
        ("another-region", other_region_arn.as_str()),
    ];

    for (target, cluster_arn) in unresolvable {
        // The version a request against an unresolvable cluster submits cannot be
        // that cluster's, since there is none; it only has to be present.
        for row in rows("K3AEGXETSR30VB", &channel_arn) {
            let case = format!("{target}: {}", row.case);
            let reply = send(&emulator, cluster_arn, &row).await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (404, "NotFoundException"),
                "case {case}: expected 404 NotFoundException for a cluster this endpoint does \
                 not hold, got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.error_message(),
                RESOURCE_NOT_FOUND_MESSAGE,
                "case {case}: expected the service's fixed not-found sentence, got {:?}",
                reply.error_message()
            );
        }
    }

    // The SAME requests against the cluster this endpoint does hold are refused as
    // client-input faults instead — so the member values the rows carry really are
    // refusable, and the not-found answers above were the cluster gate rather than
    // a refusal that never reaches the member.
    for row in rows(&live_version, &channel_arn) {
        let case = format!("live: {}", row.case);
        let reply = send(&emulator, &live_arn, &row).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case}: expected 400 BadRequestException once the cluster resolves, got {} {}",
            reply.status,
            reply.body
        );
        if let Some(member) = row.member {
            assert_eq!(
                reply.error_message(),
                BAD_REQUEST_MESSAGE,
                "case {case}: expected the service's fixed parameter sentence, got {:?}",
                reply.error_message()
            );
            assert_eq!(
                reply.at("/invalidParameter").as_str(),
                Some(member),
                "case {case}: expected the refusal to name {member}, got {}",
                reply.body
            );
        }
    }

    // None of those refusals touched the cluster: it is still ACTIVE, at the
    // version it was, with no operation in flight.
    let after = emulator.describe(&live_arn).await;
    assert_eq!(
        after.at("/clusterInfo/state").as_str(),
        Some("ACTIVE"),
        "a refused request must leave the cluster ACTIVE, got {}",
        after.body
    );
    assert_eq!(
        after.at("/clusterInfo/currentVersion").as_str(),
        Some(live_version.as_str()),
        "a refused request must not move the cluster's version, got {}",
        after.at("/clusterInfo/currentVersion")
    );
    assert_eq!(
        after.at("/clusterInfo/activeOperationArn"),
        &Value::Null,
        "a refused request must start no cluster operation, got {}",
        after.at("/clusterInfo/activeOperationArn")
    );

    // And the request that is wrong in NOTHING is accepted on that same cluster,
    // which is what makes every refusal above about what the row changed.
    let accepted = emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/nodes/count", pct(&live_arn)),
            Some(json!({
                "currentVersion": live_version,
                "targetNumberOfBrokerNodes": i64::from(MAX_BROKER_NODES_PER_CLUSTER),
            })),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "expected a target inside the published bounds to be accepted, got {} {}",
        accepted.status, accepted.body
    );
}
