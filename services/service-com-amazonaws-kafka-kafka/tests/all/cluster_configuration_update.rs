// SPDX-License-Identifier: CC0-1.0

//! `UpdateClusterConfiguration` end to end, over the same `MultiServiceRouter` the
//! emulator serves from, through the harness in `tests/all/kafka_harness.rs`.
//!
//! The rules under test, each asserted over a TABLE rather than one request:
//!
//! 1. an accepted update is applied ASYNCHRONOUSLY: it records a cluster
//!    operation of the cluster-configuration type, leaves the cluster `UPDATING`
//!    with that operation as its `ActiveOperationArn` and still its previous
//!    configuration, and lands the named revision plus a new `CurrentVersion`
//!    only once the clock owes the operation its completion — every other member
//!    of the cluster untouched;
//! 2. the `ConfigurationInfo` must resolve to a revision this endpoint's region
//!    holds, otherwise the call is the configuration family's one
//!    unresolvable-reference refusal and the cluster is left exactly as it was;
//! 3. the cluster the URI names is resolved before the body: an ARN this region
//!    holds no cluster for is a `NotFoundException` whatever the body asks for,
//!    and an ARN that is not a cluster ARN at all a `BadRequestException`;
//! 4. the submitted `CurrentVersion` must be the one the cluster is at, the
//!    cluster must be `ACTIVE`, and a serverless cluster has no broker
//!    configuration of its own to update.
//!
//! The rows differ along what each rule generalises over: which revision of a
//! configuration is applied (its first, a middle one, its latest), which
//! configuration it is and what its properties hold, whether the cluster already
//! runs one, the kind of unresolvable reference, the kind of ARN, the kind of
//! stale version, and the cluster's state and kind.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    BAD_REQUEST_MESSAGE, CLUSTER_ARN_MEMBER, OPERATION_STATE_COMPLETE, OPERATION_STATE_IN_PROGRESS,
    OPERATION_TYPE_UPDATE_CLUSTER_CONFIGURATION, PROVISIONED_ONLY_MESSAGE,
    RESOURCE_NOT_FOUND_MESSAGE,
};

use crate::kafka_harness::{
    assert_cluster_state_refusal, assert_configuration_arn_resolves_to_nothing, pct,
    sleep_past_activation, sleep_past_update, ClusterRequest, ConfigurationRequest, Emulator, Reply,
    ACCOUNT, REGION,
};

/// `PUT /v1/clusters/{ClusterArn}/configuration` — the route this suite drives,
/// with `body` as the row gives it.
async fn update_configuration(emulator: &Emulator, cluster_arn: &str, body: Value) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/configuration", pct(cluster_arn)),
            Some(body),
        )
        .await
}

/// The same call with the two required body members, which is what every row
/// sends.
async fn apply_revision(
    emulator: &Emulator,
    cluster_arn: &str,
    current_version: &str,
    configuration_arn: &str,
    revision: i64,
) -> Reply {
    update_configuration(
        emulator,
        cluster_arn,
        json!({
            "currentVersion": current_version,
            "configurationInfo": { "arn": configuration_arn, "revision": revision },
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

/// A provisioned cluster waited into `ACTIVE`, as (ARN, `CurrentVersion`): the
/// state an update is accepted from.
async fn active_cluster(emulator: &Emulator, name: &str) -> (String, String) {
    let arn = emulator.create_ok(&ClusterRequest::named(name)).await;
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

/// The `CurrentVersion` a cluster is at now.
async fn current_version(emulator: &Emulator, cluster_arn: &str) -> String {
    emulator
        .describe(cluster_arn)
        .await
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The configuration a cluster reports its brokers running, as the (ARN,
/// revision) pair `ClusterInfo$CurrentBrokerSoftwareInfo` carries.
async fn reported_configuration(
    emulator: &Emulator,
    cluster_arn: &str,
) -> (Option<String>, Option<i64>) {
    let described = emulator.describe(cluster_arn).await;
    (
        described
            .at("/clusterInfo/currentBrokerSoftwareInfo/configurationArn")
            .as_str()
            .map(str::to_string),
        described
            .at("/clusterInfo/currentBrokerSoftwareInfo/configurationRevision")
            .as_i64(),
    )
}

/// RULE: an accepted `UpdateClusterConfiguration` does not rewrite the cluster
/// where it stands — it starts a cluster operation, so until the clock owes that
/// operation its completion the cluster reads `UPDATING` with the operation as
/// its `ActiveOperationArn` and still its previous configuration and
/// `CurrentVersion`; once it lands the cluster is `ACTIVE` again running the
/// named revision at a NEW `CurrentVersion`, with every other member of the
/// record unchanged, and the operation itself reports the configuration it moved
/// from and to.
#[tokio::test]
async fn an_update_lands_the_named_revision_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    // A configuration holding THREE revisions, so a row can name its first, a
    // middle one and its latest, and a second configuration whose properties are
    // bytes that are not text at all — a revision is applied by reference, so
    // what it holds does not change how it lands.
    let three = emulator
        .create_configuration_row_ok_in(
            REGION,
            &ConfigurationRequest {
                extra_revisions: vec![b"num.partitions=4\n".to_vec(), b"num.partitions=8\n".to_vec()],
                ..ConfigurationRequest::named("three-revisions")
            },
        )
        .await;
    let binary = emulator
        .create_configuration_row_ok_in(
            REGION,
            &ConfigurationRequest {
                server_properties: [
                    b"compression.type=".as_slice(),
                    &[0x00, 0xff, 0xfe, 0x10, 0x80],
                ]
                .concat(),
                ..ConfigurationRequest::named("binary-properties")
            },
        )
        .await;

    // The rows differ along what the rule generalises over: which revision of a
    // configuration is named, and which configuration it belongs to.
    let rows: &[(&str, &str, i64)] = &[
        ("first-revision", three.as_str(), 1),
        ("middle-revision", three.as_str(), 2),
        ("latest-revision", three.as_str(), 3),
        ("other-configuration", binary.as_str(), 1),
    ];

    for (index, (case, configuration_arn, revision)) in rows.iter().enumerate() {
        let (arn, version) = active_cluster(&emulator, &format!("applied-{index}")).await;
        let before = emulator.describe(&arn).await;
        let before = before.at("/clusterInfo").clone();

        let reply = apply_revision(&emulator, &arn, &version, configuration_arn, *revision).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected UpdateClusterConfiguration to return 200, got {} {}",
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
                panic!("case {case}: UpdateClusterConfiguration returned no clusterOperationArn")
            })
            .to_string();
        assert!(
            operation_arn
                .starts_with(&format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/")),
            "case {case}: expected a cluster-operation ARN of this region, got {operation_arn:?}"
        );

        // Still rolling: the operation is the cluster's active one and nothing it
        // will change has changed yet — except the VERSION, which the accepted
        // update generated, so it is already the one a further change must quote.
        let rolling = emulator.describe(&arn).await;
        assert_eq!(
            (
                rolling.at("/clusterInfo/state").as_str(),
                rolling.at("/clusterInfo/activeOperationArn").as_str(),
            ),
            (Some("UPDATING"), Some(operation_arn.as_str())),
            "case {case}: expected the cluster to read UPDATING on the new operation, got {}",
            rolling.body
        );
        assert_ne!(
            rolling.at("/clusterInfo/currentVersion").as_str(),
            Some(version.as_str()),
            "case {case}: an accepted update generates the cluster's next version, got {}",
            rolling.body
        );
        assert_eq!(
            reported_configuration(&emulator, &arn).await,
            (None, None),
            "case {case}: the configuration must not change before the operation lands"
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
            (
                operation.at("/clusterOperationInfo/operationType").as_str(),
                operation
                    .at("/clusterOperationInfo/operationState")
                    .as_str(),
                operation.at("/clusterOperationInfo/clusterArn").as_str(),
            ),
            (
                Some(OPERATION_TYPE_UPDATE_CLUSTER_CONFIGURATION),
                Some(OPERATION_STATE_IN_PROGRESS),
                Some(arn.as_str()),
            ),
            "case {case}: expected an in-progress cluster-configuration operation on this \
             cluster, got {}",
            operation.body
        );
        assert_eq!(
            (
                operation
                    .at("/clusterOperationInfo/targetClusterInfo/configurationInfo/arn")
                    .as_str(),
                operation
                    .at("/clusterOperationInfo/targetClusterInfo/configurationInfo/revision")
                    .as_i64(),
            ),
            (Some(*configuration_arn), Some(*revision)),
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
            reported_configuration(&emulator, &arn).await,
            (Some((*configuration_arn).to_string()), Some(*revision)),
            "case {case}: expected the named revision to land on the cluster"
        );
        let new_version = after
            .at("/clusterInfo/currentVersion")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: DescribeCluster reported no currentVersion"))
            .to_string();
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
        // update reconfigures the brokers, it does not rewrite the record.
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

    // A cluster already running a configuration is re-pointed by the next
    // update, which reports the one it is moving away from as its source.
    let (arn, version) = active_cluster(&emulator, "repointed").await;
    let first = apply_revision(&emulator, &arn, &version, &three, 2).await;
    assert_eq!(
        first.status, 200,
        "case repointed: expected the first update to be accepted, got {} {}",
        first.status, first.body
    );
    sleep_past_update();
    let version = current_version(&emulator, &arn).await;
    let second = apply_revision(&emulator, &arn, &version, &binary, 1).await;
    assert_eq!(
        second.status, 200,
        "case repointed: expected the second update to be accepted, got {} {}",
        second.status, second.body
    );
    let operation_arn = second.body["clusterOperationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case repointed: the second update returned no operation ARN"))
        .to_string();
    let operation = describe_operation(&emulator, &operation_arn).await;
    assert_eq!(
        (
            operation
                .at("/clusterOperationInfo/sourceClusterInfo/configurationInfo/arn")
                .as_str(),
            operation
                .at("/clusterOperationInfo/sourceClusterInfo/configurationInfo/revision")
                .as_i64(),
        ),
        (Some(three.as_str()), Some(2)),
        "case repointed: the operation must report the configuration the cluster ran, got {}",
        operation.at("/clusterOperationInfo/sourceClusterInfo")
    );
    sleep_past_update();
    assert_eq!(
        reported_configuration(&emulator, &arn).await,
        (Some(binary.clone()), Some(1)),
        "case repointed: the second update must leave the cluster on the revision it named"
    );
}

/// RULE: the `ConfigurationInfo` names the revision the brokers are to run, so it
/// must resolve to a revision this endpoint's region holds — an `Arn` that
/// resolves to no configuration of this region and a revision number the
/// configuration it names never issued are alike the configuration family's one
/// unresolvable-reference refusal, and the cluster is left exactly as it was.
#[tokio::test]
async fn only_a_revision_this_region_holds_is_applied() {
    let emulator = Emulator::new();
    // A configuration holding THREE revisions, so a row can name each published
    // bound and one past it.
    let three = emulator
        .create_configuration_row_ok_in(
            REGION,
            &ConfigurationRequest {
                extra_revisions: vec![b"num.partitions=4\n".to_vec(), b"num.partitions=8\n".to_vec()],
                ..ConfigurationRequest::named("resolution-rows")
            },
        )
        .await;
    // A configuration of ANOTHER region: minted, and still not this endpoint's.
    let elsewhere = emulator
        .create_configuration_row_ok_in(
            "eu-west-1",
            &ConfigurationRequest::named("resolution-elsewhere"),
        )
        .await;
    let absent = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/absent/11111111-2222-4333-8444-555555555555-1"
    );

    // Rows: the revisions the configuration holds (its first, a middle one, its
    // latest) are applied; each bound passed, a revision number no configuration
    // issues, an ARN that resolves to nothing, one of another region, and an ARN
    // that is not a configuration ARN at all are refused.
    let rows: &[(&str, &str, i64, bool)] = &[
        ("first-revision", three.as_str(), 1, true),
        ("middle-revision", three.as_str(), 2, true),
        ("latest-revision", three.as_str(), 3, true),
        ("past-the-latest", three.as_str(), 4, false),
        ("below-the-first", three.as_str(), 0, false),
        ("negative-revision", three.as_str(), -1, false),
        ("absent-configuration", absent.as_str(), 1, false),
        ("another-region", elsewhere.as_str(), 1, false),
        ("not-an-arn", "not-an-arn", 1, false),
    ];

    for (index, (case, configuration_arn, revision, accepted)) in rows.iter().enumerate() {
        let (arn, version) = active_cluster(&emulator, &format!("resolution-{index}")).await;
        let reply = apply_revision(&emulator, &arn, &version, configuration_arn, *revision).await;

        if !accepted {
            assert_configuration_arn_resolves_to_nothing(&reply, case, configuration_arn, REGION);
            let after = emulator.describe(&arn).await;
            assert_eq!(
                (
                    after.at("/clusterInfo/state").as_str(),
                    after.at("/clusterInfo/currentVersion").as_str(),
                ),
                (Some("ACTIVE"), Some(version.as_str())),
                "case {case}: a refused update must leave the cluster exactly as it was, got {}",
                after.body
            );
            assert_eq!(
                after.at("/clusterInfo/activeOperationArn"),
                &Value::Null,
                "case {case}: a refused update must start no operation, got {}",
                after.at("/clusterInfo/activeOperationArn")
            );
            assert_eq!(
                reported_configuration(&emulator, &arn).await,
                (None, None),
                "case {case}: a refused update must apply no configuration"
            );
            continue;
        }

        assert_eq!(
            reply.status, 200,
            "case {case}: expected 200 for a revision the configuration holds, got {} {}",
            reply.status, reply.body
        );
        sleep_past_update();
        assert_eq!(
            reported_configuration(&emulator, &arn).await,
            (Some((*configuration_arn).to_string()), Some(*revision)),
            "case {case}: expected the revision the configuration holds to land"
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
    let (present, version) = active_cluster(&emulator, "resolution-order").await;
    // A real configuration of this region, so a row's body is itself valid.
    let configuration = emulator
        .create_configuration_row_ok_in(REGION, &ConfigurationRequest::named("ghost-cluster"))
        .await;
    // A cluster of the same shape in another region: well formed, and still not
    // this endpoint's.
    let elsewhere = emulator
        .create_ok_in("eu-west-1", &ClusterRequest::named("elsewhere"))
        .await;

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
    // Cluster REFERENCES that identify no cluster: each resolves to no resource a
    // permission could be scoped to, so authorization refuses them before the
    // cluster is resolved and before the body is read at all.
    let unauthorized: Vec<String> = vec![
        format!("arn:aws:kafka::{ACCOUNT}:cluster/present/abc-1"),
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/present/abc-1/extra"),
    ];
    // Each ARN is tried with a body that is itself valid and with bodies whose
    // members would each be refused on a cluster that did exist.
    let bodies: &[(&str, Value)] = &[
        (
            "valid-body",
            json!({
                "currentVersion": version,
                "configurationInfo": { "arn": configuration.as_str(), "revision": 1 },
            }),
        ),
        (
            "stale-version",
            json!({
                "currentVersion": "K0000000000000",
                "configurationInfo": { "arn": configuration.as_str(), "revision": 1 },
            }),
        ),
        (
            "unresolvable-configuration",
            json!({
                "currentVersion": version,
                "configurationInfo": { "arn": configuration.as_str(), "revision": 99 },
            }),
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
            let reply = update_configuration(&emulator, arn, body.clone()).await;
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
            // condition it answers; the authorization refusal names neither a
            // member nor what the registry holds.
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
/// `ACTIVE`, and a serverless cluster (which has no brokers of its own to
/// configure) are each refused `BadRequestException`, and nothing is persisted.
#[tokio::test]
async fn a_stale_version_or_an_unupdatable_cluster_is_refused() {
    let emulator = Emulator::new();
    let configuration = emulator
        .create_configuration_row_ok_in(REGION, &ConfigurationRequest::named("guard-rows"))
        .await;
    let (arn, version) = active_cluster(&emulator, "guards").await;

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
        let reply = apply_revision(&emulator, &arn, stale, &configuration, 1).await;
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
    assert_eq!(
        reported_configuration(&emulator, &arn).await,
        (None, None),
        "a stale quote must apply no configuration"
    );

    // A cluster still being provisioned has no settled configuration to update.
    let creating = emulator
        .create_ok(&ClusterRequest::named("still-creating"))
        .await;
    let creating_version = current_version(&emulator, &creating).await;
    let reply = apply_revision(
        &emulator,
        &creating,
        &creating_version,
        &configuration,
        1,
    )
    .await;
    // The refusal is about the STATE the cluster is in, which the service words in
    // one sentence naming the states a change is accepted from.
    assert_cluster_state_refusal(&reply, "creating");
    sleep_past_activation();
    assert_eq!(
        reported_configuration(&emulator, &creating).await,
        (None, None),
        "case creating: a refused update must apply no configuration"
    );

    // A serverless cluster has no broker nodes to configure, so there is no
    // broker configuration for this operation to move.
    let serverless = emulator
        .create_serverless_ok(
            "serverless-configuration",
            &[vec!["subnet-a", "subnet-b"]],
            Some(true),
        )
        .await;
    sleep_past_activation();
    let reply = apply_revision(
        &emulator,
        &serverless,
        "K0000000000000",
        &configuration,
        1,
    )
    .await;
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
