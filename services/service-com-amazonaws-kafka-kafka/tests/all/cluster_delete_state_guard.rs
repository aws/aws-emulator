// SPDX-License-Identifier: CC0-1.0

//! The cluster-state pre-conditions of the Amazon MSK cluster operations, driven
//! end to end over the same router the emulator serves from.
//!
//! The RULE under test: an operation whose pre-condition is the cluster's own
//! state is refused with HTTP 400 `BadRequestException` naming `ClusterArn` while
//! the cluster is not in a state that operation is accepted from, and the refusal
//! changes nothing. Two pre-conditions, one per kind of operation:
//!
//! * a cluster DELETE is accepted only from a state the service is not already
//!   moving the cluster out of on its own — so a cluster being provisioned, or
//!   carrying an update still in flight, refuses it, and the same delete succeeds
//!   once the clock has settled the cluster;
//! * a broker-level READ (`GetBootstrapBrokers`, `ListNodes`) is accepted once the
//!   cluster HAS brokers — so a cluster still being provisioned refuses it, while
//!   an update in flight, which does not take the brokers away, does not.
//!
//! The rows differ along what the rules generalise over: the transitional STATE
//! the cluster is in (`CREATING` from a create, `UPDATING` from each update
//! operation that starts one), the KIND of cluster (v1 provisioned, v2
//! provisioned, v2 serverless), its SIZE (broker count), the OPERATION asked for,
//! and what the refused request carries (`currentVersion` absent, the cluster's
//! own, and a stale one) — a state refusal is the state's, not one request
//! shape's.

use serde_json::{json, Value};

use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator, Reply,
};

/// `CreateClusterV2`'s URI, per the model's `http` binding on that operation.
const CREATE_V2_URI: &str = "/api/v2/clusters";

/// A `currentVersion` no cluster of this registry is at: the version token is `K`
/// plus 13 upper-case alphanumerics, and no cluster is ever minted at this one.
const STALE_VERSION: &str = "K0000000000000";

/// The wording Amazon MSK answers a cluster delete it refuses on the cluster's
/// state with: one fixed sentence whose only variable part is the state the
/// cluster is actually in, so the caller learns why the call was refused.
fn refusal_message(state: &str) -> String {
    format!("You can't delete cluster in {state} state.")
}

/// `DeleteCluster`: `DELETE /v1/clusters/{ClusterArn}`, with `CurrentVersion` on
/// the `currentVersion` query parameter when the row supplies one.
async fn delete(emulator: &Emulator, arn: &str, current_version: Option<&str>) -> Reply {
    let uri = match current_version {
        Some(version) => format!("/v1/clusters/{}?currentVersion={}", pct(arn), pct(version)),
        None => format!("/v1/clusters/{}", pct(arn)),
    };
    emulator.call("DELETE", &uri, None).await
}

/// `DescribeClusterV2`: `GET /api/v2/clusters/{ClusterArn}` — the reader that
/// serves a cluster of either kind, so one row shape covers both.
async fn describe_v2(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/api/v2/clusters/{}", pct(arn)), None)
        .await
}

/// The state `DescribeClusterV2` reports for `arn`, or `""` when it reports none.
async fn reported_state(emulator: &Emulator, arn: &str) -> String {
    describe_v2(emulator, arn)
        .await
        .at("/clusterInfo/state")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Whether `ListClustersV2` still lists `arn` — a refused delete must leave the
/// registry exactly as it was, which the listing is the second witness of.
async fn is_listed(emulator: &Emulator, arn: &str) -> bool {
    emulator
        .list_v2("")
        .await
        .at("/clusterInfoList")
        .as_array()
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["clusterArn"].as_str() == Some(arn))
        })
}

/// Assert ONE refused delete: the status, the modelled error code, the parameter
/// it names and the wording, and that the cluster is untouched afterwards.
async fn assert_delete_refused(
    emulator: &Emulator,
    case: &str,
    arn: &str,
    current_version: Option<&str>,
    state: &str,
) {
    let reply = delete(emulator, arn, current_version).await;
    assert_eq!(
        reply.status, 400,
        "case {case}: expected HTTP 400 from DeleteCluster in {state}, got {} {}",
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
        Some("ClusterArn"),
        "case {case}: expected the refusal to name ClusterArn, got {}",
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        refusal_message(state),
        "case {case}: expected the state refusal wording, got {}",
        reply.body
    );
    // The refusal had no effect: the cluster is still readable, still in the state
    // it was in, and still listed.
    assert_eq!(
        reported_state(emulator, arn).await,
        state,
        "case {case}: a refused delete must leave the cluster in {state}"
    );
    assert!(
        is_listed(emulator, arn).await,
        "case {case}: a refused delete must leave the cluster listed"
    );
}

/// Assert the delete `arn` now accepts: 200, the cluster's own ARN, `DELETING`,
/// and every later reader answering as if the cluster never existed.
async fn assert_delete_accepted(emulator: &Emulator, case: &str, arn: &str) {
    let reply = delete(emulator, arn, None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected HTTP 200 from DeleteCluster once the cluster settled, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        (
            reply.body["clusterArn"].as_str(),
            reply.body["state"].as_str()
        ),
        (Some(arn), Some("DELETING")),
        "case {case}: expected the deleted cluster reported DELETING, got {}",
        reply.body
    );
    let described = describe_v2(emulator, arn).await;
    assert_eq!(
        (described.status, described.error_code()),
        (404, "NotFoundException"),
        "case {case}: expected a 404 after the accepted delete, got {} {}",
        described.status,
        described.body
    );
}

/// The kind of create a row mints its cluster with. All three land in the one
/// cluster registry, so all three are deleted by the same operation.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// `CreateCluster` (`POST /v1/clusters`).
    V1Provisioned,
    /// `CreateClusterV2` with a `Provisioned` body.
    V2Provisioned,
    /// `CreateClusterV2` with a `Serverless` body.
    V2Serverless,
}

/// One row of the `CREATING` table: the cluster to mint, and how.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    kind: Kind,
    /// Broker count, so the rule is not keyed to one size of cluster. Ignored by
    /// a serverless row, which has no broker nodes.
    brokers: i64,
}

/// Mint this row's cluster and hand back its ARN, asserting the create reported
/// the transient `CREATING` state the guard below is about.
async fn create(emulator: &Emulator, row: &Row) -> String {
    let reply = match row.kind {
        Kind::V1Provisioned => {
            let request = ClusterRequest {
                brokers: row.brokers,
                ..ClusterRequest::named(row.case)
            };
            emulator.create(&request).await
        }
        Kind::V2Provisioned => {
            let request = ClusterRequest {
                brokers: row.brokers,
                ..ClusterRequest::named(row.case)
            };
            // The v2 `ProvisionedRequest` carries the v1 members apart from the
            // cluster name, which stays at the top level.
            let mut provisioned = request.to_json();
            let name = provisioned
                .as_object_mut()
                .expect("the CreateCluster body is a JSON object")
                .remove("clusterName")
                .expect("the CreateCluster body names the cluster");
            emulator
                .call(
                    "POST",
                    CREATE_V2_URI,
                    Some(json!({ "clusterName": name, "provisioned": provisioned })),
                )
                .await
        }
        Kind::V2Serverless => {
            emulator
                .call(
                    "POST",
                    CREATE_V2_URI,
                    Some(json!({
                        "clusterName": row.case,
                        "serverless": { "vpcConfigs": [{ "subnetIds": ["subnet-0a1b2c3d"] }] },
                    })),
                )
                .await
        }
    };
    assert_eq!(
        reply.status, 200,
        "case {}: expected the create to return 200, got {} {}",
        row.case, reply.status, reply.body
    );
    assert_eq!(
        reply.body["state"].as_str(),
        Some("CREATING"),
        "case {}: expected the create to report CREATING, got {}",
        row.case,
        reply.body
    );
    reply.body["clusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {}: the create returned no clusterArn", row.case))
        .to_string()
}

/// RULE: a cluster delete is accepted only from a state the service is not
/// already moving the cluster out of, so a cluster still being provisioned
/// refuses it with HTTP 400 `BadRequestException` and stays exactly where it was
/// — whatever the request carries beside the ARN — and the same delete succeeds
/// once the clock has settled the cluster.
#[tokio::test]
async fn a_delete_is_refused_while_the_cluster_is_still_being_created() {
    // Rows differ along the kind of cluster and its size; the state they are all
    // in at the moment of the delete is the one the rule is about.
    let rows = [
        Row {
            case: "v1-three-brokers",
            kind: Kind::V1Provisioned,
            brokers: 3,
        },
        Row {
            case: "v1-single-broker",
            kind: Kind::V1Provisioned,
            brokers: 1,
        },
        Row {
            case: "v1-fifteen-brokers",
            kind: Kind::V1Provisioned,
            brokers: 15,
        },
        Row {
            case: "v2-provisioned",
            kind: Kind::V2Provisioned,
            brokers: 2,
        },
        Row {
            case: "v2-serverless",
            kind: Kind::V2Serverless,
            brokers: 0,
        },
    ];

    let emulator = Emulator::new();
    let mut minted = Vec::new();
    for row in &rows {
        let arn = create(&emulator, row).await;
        assert_eq!(
            reported_state(&emulator, &arn).await,
            "CREATING",
            "case {}: the cluster must read CREATING before the delete is probed",
            row.case
        );

        // What the request carries beside the ARN does not change the answer: no
        // `currentVersion`, the cluster's own, and a stale one are all refused on
        // the state.
        let version = describe_v2(&emulator, &arn)
            .await
            .at("/clusterInfo/currentVersion")
            .as_str()
            .map(str::to_string);
        let mut probes: Vec<Option<&str>> = vec![None, Some(STALE_VERSION)];
        if let Some(version) = version.as_deref() {
            probes.push(Some(version));
        }
        for probe in probes {
            let case = format!("{} currentVersion={probe:?}", row.case);
            assert_delete_refused(&emulator, &case, &arn, probe, "CREATING").await;
        }
        minted.push(arn);
    }

    // The refusals changed nothing, so every cluster still settles ACTIVE on its
    // own — never DELETING — and is deletable then.
    sleep_past_activation();
    for (row, arn) in rows.iter().zip(&minted) {
        assert_eq!(
            reported_state(&emulator, arn).await,
            "ACTIVE",
            "case {}: a cluster whose delete was refused must still settle ACTIVE",
            row.case
        );
        assert_delete_accepted(&emulator, row.case, arn).await;
    }
}

/// One row of the broker-level read table: the operation, its URI suffix under
/// the cluster, and the action its state refusal names.
struct BrokerRead {
    case: &'static str,
    suffix: &'static str,
    /// The refusal this read answers while the cluster is still `CREATING`, as a
    /// template over the state — each operation words it its own way.
    refusal: fn(&str) -> String,
}

/// RULE: a cluster the service is still provisioning has no broker nodes yet, so
/// an operation whose answer IS those brokers is refused with HTTP 400
/// `BadRequestException` naming `ClusterArn` while the cluster is `CREATING`, and
/// answers once the cluster has them — including while a later update is in
/// flight, which does not take the brokers away.
#[tokio::test]
async fn the_broker_level_reads_are_refused_until_the_cluster_has_brokers() {
    // Rows: every reader of this crate whose answer is the cluster's brokers.
    let reads = [
        BrokerRead {
            case: "GetBootstrapBrokers",
            suffix: "/bootstrap-brokers",
            refusal: |state| {
                format!("You can't get bootstrap broker nodes for a cluster in {state} state.")
            },
        },
        BrokerRead {
            case: "ListNodes",
            suffix: "/nodes",
            refusal: |state| {
                format!("You can't list the nodes for a cluster that is in the {state} state.")
            },
        },
    ];
    // Cluster rows, so the refusal is not keyed to one shape of cluster.
    let clusters = [
        Row {
            case: "reads-v1-three-brokers",
            kind: Kind::V1Provisioned,
            brokers: 3,
        },
        Row {
            case: "reads-v1-single-broker",
            kind: Kind::V1Provisioned,
            brokers: 1,
        },
        Row {
            case: "reads-v2-provisioned",
            kind: Kind::V2Provisioned,
            brokers: 2,
        },
    ];

    let emulator = Emulator::new();
    let mut minted = Vec::new();
    for row in &clusters {
        let arn = create(&emulator, row).await;
        for read in &reads {
            let case = format!("{} {}", row.case, read.case);
            let reply = emulator
                .call("GET", &format!("/v1/clusters/{}{}", pct(&arn), read.suffix), None)
                .await;
            assert_eq!(
                reply.status, 400,
                "case {case}: expected HTTP 400 while the cluster is CREATING, got {} {}",
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
                Some("ClusterArn"),
                "case {case}: expected the refusal to name ClusterArn, got {}",
                reply.body
            );
            assert_eq!(
                reply.error_message(),
                (read.refusal)("CREATING"),
                "case {case}: expected the state refusal wording, got {}",
                reply.body
            );
        }
        minted.push(arn);
    }

    sleep_past_activation();
    for (row, arn) in clusters.iter().zip(&minted) {
        for read in &reads {
            let case = format!("{} {}", row.case, read.case);
            let reply = emulator
                .call("GET", &format!("/v1/clusters/{}{}", pct(arn), read.suffix), None)
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case}: expected HTTP 200 once the cluster has its brokers, got {} {}",
                reply.status, reply.body
            );
        }
    }

    // An update does not take the brokers away, so the same reads keep answering
    // while one is in flight: the pre-condition is that the cluster HAS brokers,
    // not that nothing is being applied to them.
    let arn = minted
        .first()
        .expect("the table minted at least one cluster")
        .clone();
    let version = describe_v2(&emulator, &arn)
        .await
        .at("/clusterInfo/currentVersion")
        .as_str()
        .expect("a settled cluster reports a currentVersion")
        .to_string();
    let updated = emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/storage", pct(&arn)),
            Some(json!({ "currentVersion": version, "volumeSizeGB": 150 })),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "expected the update to be accepted, got {} {}",
        updated.status, updated.body
    );
    assert_eq!(
        reported_state(&emulator, &arn).await,
        "UPDATING",
        "an accepted update must leave the cluster UPDATING"
    );
    for read in &reads {
        let reply = emulator
            .call("GET", &format!("/v1/clusters/{}{}", pct(&arn), read.suffix), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {} while UPDATING: expected HTTP 200, got {} {}",
            read.case, reply.status, reply.body
        );
    }
}

/// One row of the `UPDATING` table: an update operation that puts an `ACTIVE`
/// cluster into `UPDATING`, as (case, URI suffix, body built from the version).
struct UpdateRow {
    case: &'static str,
    suffix: &'static str,
    body: fn(&str) -> Value,
}

/// RULE: the same pre-condition holds for the other state the service moves a
/// cluster through on its own — a cluster with an accepted update still in flight
/// refuses the delete, and accepts it once that update has landed.
#[tokio::test]
async fn a_delete_is_refused_while_an_update_is_in_flight() {
    // Rows: every update operation of this crate that puts the cluster into
    // `UPDATING`, so the refusal is the state's rather than one update's.
    let rows = [
        UpdateRow {
            case: "update-storage",
            suffix: "/storage",
            body: |version| json!({ "currentVersion": version, "volumeSizeGB": 200 }),
        },
        UpdateRow {
            case: "update-broker-storage",
            suffix: "/nodes/storage",
            body: |version| {
                json!({
                    "currentVersion": version,
                    // `BrokerEBSVolumeInfo$KafkaBrokerNodeId` documents the
                    // keyword ALL: the change applies to every broker.
                    "targetBrokerEBSVolumeInfo": [
                        { "kafkaBrokerNodeId": "ALL", "volumeSizeGB": 300 }
                    ],
                })
            },
        },
        UpdateRow {
            case: "update-broker-type",
            suffix: "/nodes/type",
            body: |version| {
                json!({ "currentVersion": version, "targetInstanceType": "kafka.m5.xlarge" })
            },
        },
        UpdateRow {
            case: "update-connectivity",
            suffix: "/connectivity",
            body: |version| {
                json!({
                    "currentVersion": version,
                    "connectivityInfo": { "publicAccess": { "type": "DISABLED" } },
                })
            },
        },
    ];

    let emulator = Emulator::new();
    for row in &rows {
        let arn = emulator.create_ok(&ClusterRequest::named(row.case)).await;
        sleep_past_activation();
        let described = describe_v2(&emulator, &arn).await;
        assert_eq!(
            described.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {}: the cluster must be ACTIVE before the update, got {}",
            row.case,
            described.body
        );
        let version = described
            .at("/clusterInfo/currentVersion")
            .as_str()
            .unwrap_or_else(|| panic!("case {}: the cluster reported no currentVersion", row.case))
            .to_string();

        let updated = emulator
            .call(
                "PUT",
                &format!("/v1/clusters/{}{}", pct(&arn), row.suffix),
                Some((row.body)(&version)),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {}: expected the update to be accepted, got {} {}",
            row.case, updated.status, updated.body
        );
        assert_eq!(
            reported_state(&emulator, &arn).await,
            "UPDATING",
            "case {}: an accepted update must leave the cluster UPDATING",
            row.case
        );

        assert_delete_refused(&emulator, row.case, &arn, None, "UPDATING").await;

        // The update lands because the clock came due, not because of the refused
        // delete, and the settled cluster is deletable.
        sleep_past_update();
        assert_eq!(
            reported_state(&emulator, &arn).await,
            "ACTIVE",
            "case {}: the in-flight update must still land",
            row.case
        );
        assert_delete_accepted(&emulator, row.case, &arn).await;
    }
}

/// What Amazon MSK answers a call that CHANGES a cluster which is not in a state
/// that call is accepted from: one fixed sentence naming the states the operation
/// IS valid for and nothing else — no state the cluster happens to be in, and no
/// value from the request. Spelled out here rather than built from the service's own
/// accepted-state set, so this suite pins both the wording and the permitted set;
/// the space before the colon is the service's own.
const ALLOWED_STATES_REFUSAL: &str =
    "This operation is only valid for resources that are in one of the following states :[ACTIVE]";

/// One row of the state-guard wording table: the operation, how it is called, and
/// the sentence it answers while the cluster is in `state`.
struct GuardRow {
    case: &'static str,
    method: &'static str,
    suffix: &'static str,
    body: Option<fn() -> Value>,
    /// The refusal this operation answers, as a template over the state the cluster
    /// is in — each operation words its own, and the one a CHANGE answers names the
    /// accepted states instead of the current one.
    refusal: fn(&str) -> String,
    /// Whether this operation is still refused once the cluster HAS its brokers but
    /// is not settled (a reboot in flight): a change is, a broker-level read is not.
    refused_once_brokers_exist: bool,
}

/// RULE: a cluster operation whose pre-condition is the cluster's state answers its
/// OWN sentence for that refusal — the per-operation ones carry the state the
/// cluster is actually in, while a call that CHANGES the cluster answers the generic
/// sentence naming the states it is accepted from — and the code and status are the
/// same 400 `BadRequestException` naming `ClusterArn` for all of them.
#[tokio::test]
async fn each_state_guard_answers_the_sentence_its_own_operation_answers() {
    // Rows: every operation of this crate whose refusal is the cluster's state —
    // the delete, the two broker-level reads, and the broker reboot, which is a
    // change to the cluster rather than a read of it.
    let rows = [
        GuardRow {
            case: "DeleteCluster",
            method: "DELETE",
            suffix: "",
            body: None,
            refusal: |state| format!("You can't delete cluster in {state} state."),
            refused_once_brokers_exist: true,
        },
        GuardRow {
            case: "ListNodes",
            method: "GET",
            suffix: "/nodes",
            body: None,
            refusal: |state| {
                format!("You can't list the nodes for a cluster that is in the {state} state.")
            },
            refused_once_brokers_exist: false,
        },
        GuardRow {
            case: "GetBootstrapBrokers",
            method: "GET",
            suffix: "/bootstrap-brokers",
            body: None,
            refusal: |state| {
                format!("You can't get bootstrap broker nodes for a cluster in {state} state.")
            },
            refused_once_brokers_exist: false,
        },
        GuardRow {
            case: "RebootBroker",
            method: "PUT",
            suffix: "/reboot-broker",
            body: Some(|| json!({ "brokerIds": ["1"] })),
            // A change names the states it is accepted from, not the one the
            // cluster is in, so this template ignores its argument.
            refusal: |_state| ALLOWED_STATES_REFUSAL.to_string(),
            refused_once_brokers_exist: true,
        },
    ];

    let emulator = Emulator::new();

    // A cluster the service is still provisioning: it has no brokers yet, so every
    // row is refused — each with its own sentence over CREATING.
    let creating = emulator.create_ok(&ClusterRequest::named("guard-creating")).await;
    for row in &rows {
        let reply = emulator
            .call(
                row.method,
                &format!("/v1/clusters/{}{}", pct(&creating), row.suffix),
                row.body.map(|build| build()),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code(), reply.error_message()),
            (
                400,
                "BadRequestException",
                (row.refusal)("CREATING").as_str()
            ),
            "case {} while CREATING: expected its own state refusal, got {} {}",
            row.case,
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.body["invalidParameter"].as_str(),
            Some("ClusterArn"),
            "case {} while CREATING: expected the refusal to name ClusterArn, got {}",
            row.case,
            reply.body
        );
    }

    // A cluster whose brokers are being rebooted: it HAS its brokers, so the two
    // reads are served, while the delete and a second reboot are refused — the
    // delete naming the state the cluster is in and the reboot naming the states it
    // would accept.
    let rebooting = emulator.create_ok(&ClusterRequest::named("guard-rebooting")).await;
    sleep_past_activation();
    let accepted = emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/reboot-broker", pct(&rebooting)),
            Some(json!({ "brokerIds": ["1"] })),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "expected the reboot of a settled cluster to be accepted, got {} {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        reported_state(&emulator, &rebooting).await,
        "REBOOTING_BROKER",
        "an accepted reboot must leave the cluster REBOOTING_BROKER"
    );
    for row in &rows {
        let reply = emulator
            .call(
                row.method,
                &format!("/v1/clusters/{}{}", pct(&rebooting), row.suffix),
                row.body.map(|build| build()),
            )
            .await;
        if row.refused_once_brokers_exist {
            assert_eq!(
                (reply.status, reply.error_code(), reply.error_message()),
                (
                    400,
                    "BadRequestException",
                    (row.refusal)("REBOOTING_BROKER").as_str()
                ),
                "case {} while REBOOTING_BROKER: expected its own state refusal, got {} {}",
                row.case,
                reply.status,
                reply.body
            );
        } else {
            assert_eq!(
                reply.status, 200,
                "case {} while REBOOTING_BROKER: a cluster that has its brokers serves this \
                 read, got {} {}",
                row.case, reply.status, reply.body
            );
        }
    }
}
