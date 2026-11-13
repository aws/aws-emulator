// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the configuration the SERVICE puts in a domain beyond what the
//! request carried, and for what a descriptor of that domain derives at the moment of
//! the read.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness comes from [`super::domain_lifecycle`]; this suite adds no second
//! copy of it.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. `CreateDomain` provisions a domain with a WHOLE configuration: every option block
//!    the request left out, and every member of a block it filled only part of, is
//!    stored at the value the service sets for it, so `CreateDomain`, `DescribeDomain`,
//!    `DescribeDomains`, `DeleteDomain` and `DescribeDomainConfig` all report a value
//!    for every option rather than nothing. The rows differ along how much of the
//!    configuration the request names — nothing at all, one block, a block filled in
//!    part, a block whose default the request overrides — and along the volume type,
//!    which decides whether the performance members are provisioned at all.
//! 2. An EBS performance member the volume type does not provision is refused, and no
//!    domain is created.
//! 3. A configuration change waits (`Pending`) while the cluster it configures is being
//!    provisioned and is `ApplyingChanges` while it is being applied to a provisioned
//!    one, reporting the leaf properties it is applying and no stage.
//! 4. `DeleteDomain` releases the name, answers the change it is applying, and answers a
//!    domain this account does not hold with the status the service answers for one.
//! 5. A domain's health reports the cluster its CONFIGURATION describes, and only
//!    observations of a cluster that is up.

use serde_json::{json, Value};

use super::domain_lifecycle::{
    create_baseline, error_code, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// The option blocks and members the service fills in for a domain created without
/// them, with the value it sets for each. The expectations are stated here, once, and
/// every reader of a created domain is held to them.
///
/// Sources: `AdvancedOptions` documents "Default is true" for the explicit-index
/// option, `SnapshotOptions$AutomatedSnapshotStartHour` "Default is `0` hours", and
/// `OffPeakWindowOptions` that "all domains created after [February 16, 2023] have an
/// off-peak window by default"; the enumerated values are the ones
/// `T_L_S_SECURITY_POLICY_VALUES`, `DEPLOYMENT_STRATEGY_VALUES`,
/// `I_P_ADDRESS_TYPE_VALUES`, `PAUSE_STATE_VALUES`, `AUTO_TUNE_STATE_VALUES` and
/// `NATURAL_LANGUAGE_QUERY_GENERATION_CURRENT_STATE_VALUES` publish.
fn server_defaults() -> Vec<(&'static str, Value)> {
    vec![
        (
            "AIMLOptions",
            json!({
                "NaturalLanguageQueryGenerationOptions": {
                    "CurrentState": "NOT_ENABLED",
                    "DesiredState": "DISABLED",
                },
                "S3VectorsEngine": { "Enabled": false },
                "ServerlessVectorAcceleration": { "Enabled": false },
            }),
        ),
        ("AccessPolicies", json!("")),
        (
            "AdvancedOptions",
            json!({
                "override_main_response_version": "false",
                "rest.action.multi.allow_explicit_index": "true",
            }),
        ),
        (
            "AdvancedSecurityOptions",
            json!({
                "AnonymousAuthEnabled": false,
                "Enabled": false,
                "InternalUserDatabaseEnabled": false,
            }),
        ),
        (
            "AutoTuneOptions",
            json!({ "State": "DISABLED", "UseOffPeakWindow": false }),
        ),
        (
            "AutomatedSnapshotPauseOptions",
            json!({ "Enabled": false, "State": "Disabled" }),
        ),
        ("CognitoOptions", json!({ "Enabled": false })),
        (
            "DeploymentStrategyOptions",
            json!({ "DeploymentStrategy": "CapacityOptimized" }),
        ),
        (
            "DomainEndpointOptions",
            json!({
                "CustomEndpointEnabled": false,
                "EnforceHTTPS": false,
                "TLSSecurityPolicy": "Policy-Min-TLS-1-2-2019-07",
            }),
        ),
        ("EncryptionAtRestOptions", json!({ "Enabled": false })),
        ("IPAddressType", json!("ipv4")),
        ("IdentityCenterOptions", json!({})),
        ("NodeToNodeEncryptionOptions", json!({ "Enabled": false })),
        (
            "OffPeakWindowOptions",
            json!({
                "Enabled": true,
                "OffPeakWindow": { "WindowStartTime": { "Hours": 5, "Minutes": 0 } },
            }),
        ),
        ("SnapshotOptions", json!({ "AutomatedSnapshotStartHour": 0 })),
        (
            "SoftwareUpdateOptions",
            json!({
                "AutoSoftwareUpdateEnabled": false,
                "UseLatestServiceSoftwareForBlueGreen": true,
            }),
        ),
    ]
}

/// The cluster-topology toggles the service fills in, and the value it sets for each.
fn cluster_defaults() -> Vec<(&'static str, Value)> {
    vec![
        ("ColdStorageOptions", json!({ "Enabled": false })),
        ("DedicatedMasterEnabled", json!(false)),
        ("MultiAZWithStandbyEnabled", json!(false)),
        ("WarmEnabled", json!(false)),
        ("ZoneAwarenessEnabled", json!(false)),
    ]
}

/// `CreateDomain` for `name` with `body`'s members, asserting it was accepted, and the
/// `DomainStatus` it answered with.
async fn created(emulator: &Emulator, name: &str, body: Value) -> Value {
    let mut request = body;
    request["DomainName"] = json!(name);
    let answer = emulator.create(request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}, {request}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"].clone()
}

/// The `DomainConfig` `DescribeDomainConfig` reports for `name`.
async fn configured(emulator: &Emulator, name: &str) -> Value {
    let answer = emulator.describe_config(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomainConfig({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainConfig"].clone()
}

/// The `ChangeProgressStatus` of the change in flight on `name`.
async fn progress(emulator: &Emulator, name: &str) -> Value {
    let answer = emulator
        .call("GET", &format!("{PREFIX}/opensearch/domain/{name}/progress"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomainChangeProgress({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["ChangeProgressStatus"].clone()
}

/// The `DescribeDomainHealth` body for `name`.
async fn health(emulator: &Emulator, name: &str) -> Value {
    let answer = emulator
        .call("GET", &format!("{PREFIX}/opensearch/domain/{name}/health"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomainHealth({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body.clone()
}

#[tokio::test]
async fn a_created_domain_carries_the_whole_configuration_the_service_provisions_it_with() {
    let emulator = Emulator::new();
    // The rows differ along how much of the configuration the request names: nothing at
    // all, the cluster alone, a block filled in part (one of two members), a block
    // whose default the request overrides, and a block the service does not default at
    // all.
    let rows: Vec<(&str, Value)> = vec![
        ("defaults-bare", json!({})),
        (
            "defaults-cluster",
            json!({ "ClusterConfig": { "InstanceType": "r6g.large.search", "InstanceCount": 2 } }),
        ),
        (
            "defaults-partial-block",
            json!({ "DomainEndpointOptions": { "EnforceHTTPS": true } }),
        ),
        (
            "defaults-overridden",
            json!({
                "IPAddressType": "dualstack",
                "SnapshotOptions": { "AutomatedSnapshotStartHour": 7 },
                "CognitoOptions": { "Enabled": false, "UserPoolId": "us-east-1_AAAAAAAAA" },
                "AdvancedOptions": { "override_main_response_version": "true" },
            }),
        ),
        (
            "defaults-with-volume",
            json!({
                "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp2", "VolumeSize": 20 },
            }),
        ),
    ];

    for (name, request) in rows {
        let status = created(&emulator, name, request.clone()).await;
        let described = emulator.describe(name).await;
        let read_back = described.body["DomainStatus"].clone();
        let config = configured(&emulator, name).await;

        for (option, default) in server_defaults() {
            // A member the REQUEST named stands as it was sent; every other member of
            // the block is the service's own value.
            let expected = match request.get(option) {
                None => default.clone(),
                Some(sent) => {
                    let mut expected = default.clone();
                    match sent.as_object() {
                        Some(members) => {
                            for (member, value) in members {
                                expected[member] = value.clone();
                            }
                        }
                        None => expected = sent.clone(),
                    }
                    expected
                }
            };
            for (reader, reported) in [
                ("CreateDomain", &status[option]),
                ("DescribeDomain", &read_back[option]),
                ("DescribeDomainConfig", &config[option]["Options"]),
            ] {
                // `DomainStatus` reports the Auto-Tune settings through the OUTPUT
                // shape, which names the state the domain is in (`State`), and
                // `DomainConfig` through the configured one, which names the state it
                // was asked for (`DesiredState`) and carries the two settings the
                // service fills in for a domain that asked for neither — "specify
                // `NO_ROLLBACK` to retain all prior Auto-Tune settings", and the empty
                // list of maintenance schedules a domain with no schedule has.
                let expected = match (reader, option) {
                    ("DescribeDomainConfig", "AutoTuneOptions") => json!({
                        "DesiredState": expected["State"].clone(),
                        "UseOffPeakWindow": expected["UseOffPeakWindow"].clone(),
                        "MaintenanceSchedules": json!([]),
                        "RollbackOnDisable": json!("NO_ROLLBACK"),
                    }),
                    _ => expected.clone(),
                };
                assert_eq!(
                    reported, &expected,
                    "case {reader}({name:?}, {option}): expected the configuration the service \
                     provisions the domain with ({expected}), got {reported:?}"
                );
            }
        }

        // The cluster topology toggles are filled in the same way, inside the container
        // the request did or did not carry.
        for (member, default) in cluster_defaults() {
            let expected = request
                .get("ClusterConfig")
                .and_then(|cluster| cluster.get(member))
                .cloned()
                .unwrap_or(default);
            assert_eq!(
                status["ClusterConfig"][member], expected,
                "case CreateDomain({name:?}, ClusterConfig.{member}): expected {expected}, got {:?}",
                status["ClusterConfig"][member]
            );
        }

        // Every option the domain holds a value for carries an `OptionStatus`, at the
        // revision a create leaves one at: the configuration the request asked for, and
        // then the configuration the service provisions the domain with.
        for (option, _) in server_defaults() {
            assert_eq!(
                config[option]["Status"]["UpdateVersion"].as_i64(),
                Some(2),
                "case DescribeDomainConfig({name:?}, {option}): expected update version 2, got \
                 {:?}",
                config[option]["Status"]
            );
        }
        // Including the two containers every domain HAS and holds nothing in until a
        // call puts something there.
        for (option, options) in [
            ("VPCOptions", json!({})),
            ("LogPublishingOptions", json!({})),
        ] {
            assert_eq!(
                config[option]["Options"], options,
                "case DescribeDomainConfig({name:?}, {option}): expected the empty container \
                 every domain has, got {:?}",
                config[option]
            );
        }

        // The deleted domain's final status reports the same whole configuration.
        let deleted = emulator.delete(name).await;
        assert_eq!(
            deleted.status, 200,
            "case DeleteDomain({name:?}): expected 200, got {} with {:?}",
            deleted.status, deleted.body
        );
        for (option, _) in server_defaults() {
            assert_eq!(
                deleted.body["DomainStatus"][option], status[option],
                "case DeleteDomain({name:?}, {option}): expected the configuration the domain \
                 was holding, got {:?}",
                deleted.body["DomainStatus"][option]
            );
        }
    }
}

#[tokio::test]
async fn a_gp3_volume_is_provisioned_at_its_baseline_performance_and_others_are_not() {
    let emulator = Emulator::new();
    // One row per volume type the model publishes (`VOLUME_TYPE_VALUES`), with the
    // performance members the service provisions for it: `gp3` is the one type whose
    // IOPS and throughput are provisioned per volume, so it is the one that gets a
    // baseline; `io1` provisions IOPS the caller must name; the rest provision neither.
    let rows: Vec<(&str, Value, Value, Value)> = vec![
        (
            "volume-gp3-bare",
            json!({ "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 20 }),
            json!(3000),
            json!(125),
        ),
        (
            "volume-gp3-named",
            json!({
                "EBSEnabled": true,
                "VolumeType": "gp3",
                "VolumeSize": 20,
                "Iops": 4000,
                "Throughput": 250,
            }),
            json!(4000),
            json!(250),
        ),
        (
            "volume-gp2",
            json!({ "EBSEnabled": true, "VolumeType": "gp2", "VolumeSize": 20 }),
            Value::Null,
            Value::Null,
        ),
        (
            "volume-standard",
            json!({ "EBSEnabled": true, "VolumeType": "standard", "VolumeSize": 20 }),
            Value::Null,
            Value::Null,
        ),
        (
            "volume-io1",
            json!({ "EBSEnabled": true, "VolumeType": "io1", "VolumeSize": 20, "Iops": 1000 }),
            json!(1000),
            Value::Null,
        ),
    ];

    for (name, ebs, iops, throughput) in rows {
        let status = created(&emulator, name, json!({ "EBSOptions": ebs.clone() })).await;
        let read_back = emulator.describe(name).await.body["DomainStatus"].clone();
        for (reader, reported) in [
            ("CreateDomain", &status["EBSOptions"]),
            ("DescribeDomain", &read_back["EBSOptions"]),
        ] {
            assert_eq!(
                (&reported["Iops"], &reported["Throughput"]),
                (&iops, &throughput),
                "case {reader}({name:?}, {ebs}): expected Iops {iops} and Throughput \
                 {throughput}, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_performance_member_the_volume_type_does_not_provision_is_refused() {
    let emulator = Emulator::new();
    // The rows span the member that does not apply and the type it was named on:
    // throughput on each type but `gp3`, and IOPS on each type that provisions none.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "refuse-gp2-throughput",
            json!({ "EBSEnabled": true, "VolumeType": "gp2", "VolumeSize": 20, "Throughput": 125 }),
            "Throughput is only valid when volume type is GP3.",
        ),
        (
            "refuse-io1-throughput",
            json!({
                "EBSEnabled": true,
                "VolumeType": "io1",
                "VolumeSize": 20,
                "Iops": 1000,
                "Throughput": 125,
            }),
            "Throughput is only valid when volume type is GP3.",
        ),
        (
            "refuse-standard-throughput",
            json!({
                "EBSEnabled": true,
                "VolumeType": "standard",
                "VolumeSize": 20,
                "Throughput": 125,
            }),
            "Throughput is only valid when volume type is GP3.",
        ),
        (
            "refuse-gp2-iops",
            json!({ "EBSEnabled": true, "VolumeType": "gp2", "VolumeSize": 20, "Iops": 3000 }),
            "IOPS is only valid when volume type is io1 or gp3.",
        ),
        (
            "refuse-standard-iops",
            json!({ "EBSEnabled": true, "VolumeType": "standard", "VolumeSize": 20, "Iops": 3000 }),
            "IOPS is only valid when volume type is io1 or gp3.",
        ),
        (
            // A request naming no volume type gets the general-purpose type the service
            // defaults to, which provisions neither member.
            "refuse-untyped-iops",
            json!({ "EBSEnabled": true, "VolumeSize": 20, "Iops": 3000 }),
            "IOPS is only valid when volume type is io1 or gp3.",
        ),
    ];

    for (name, ebs, message) in rows {
        let answer = emulator
            .create(json!({ "DomainName": name, "EBSOptions": ebs.clone() }))
            .await;
        assert_eq!(
            answer.status, 400,
            "case CreateDomain({name:?}, {ebs}): expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case CreateDomain({name:?}, {ebs}): expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(message),
            "case CreateDomain({name:?}, {ebs}): expected the service's wording, got {:?}",
            answer.body["message"]
        );
        // A refused create leaves no domain behind, so the name is still free.
        let described = emulator.describe(name).await;
        assert_eq!(
            described.status, RECORDED_NOT_FOUND_STATUS,
            "case DescribeDomain({name:?}): a refused create stores nothing, got {} with {:?}",
            described.status, described.body
        );
    }

    // The same rule holds on an update, judged on the configuration the change would
    // leave the domain with: a volume asked to be `gp2` while carrying the throughput
    // only `gp3` provisions is refused.
    let name = "refuse-update";
    created(
        &emulator,
        name,
        json!({ "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 20 } }),
    )
    .await;
    tokio::time::sleep(SETTLE_WAIT).await;
    let answer = emulator
        .update_config(
            name,
            json!({
                "EBSOptions": {
                    "EBSEnabled": true,
                    "VolumeType": "gp2",
                    "VolumeSize": 20,
                    "Throughput": 125,
                },
            }),
        )
        .await;
    assert_eq!(
        answer.status, 400,
        "case UpdateDomainConfig({name:?}, gp2 under a provisioned throughput): expected 400, \
         got {} with {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn a_change_waits_while_the_cluster_is_provisioned_and_then_applies() {
    let emulator = Emulator::new();
    let name = "pending-change";
    let status = created(
        &emulator,
        name,
        json!({ "ClusterConfig": { "InstanceType": "r6g.large.search", "InstanceCount": 2 } }),
    )
    .await;

    // A configuration cannot be applied to a cluster that does not exist yet, so the
    // change the create starts WAITS, and says so through every descriptor of the
    // domain.
    assert_eq!(
        status["ChangeProgressDetails"]["ConfigChangeStatus"].as_str(),
        Some("Pending"),
        "case CreateDomain({name:?}): the change a create starts waits for the cluster, got {:?}",
        status["ChangeProgressDetails"]
    );
    let config = configured(&emulator, name).await;
    assert_eq!(
        config["ChangeProgressDetails"]["ConfigChangeStatus"].as_str(),
        Some("Pending"),
        "case DescribeDomainConfig({name:?}): expected the same change state, got {:?}",
        config["ChangeProgressDetails"]
    );
    let in_flight = progress(&emulator, name).await;
    assert_eq!(
        (
            in_flight["Status"].as_str(),
            in_flight["ConfigChangeStatus"].as_str(),
            in_flight["ChangeProgressStages"].as_array().map(Vec::len),
        ),
        (Some("PENDING"), Some("Pending"), Some(0)),
        "case DescribeDomainChangeProgress({name:?}): a waiting change is PENDING with no stage \
         published, got {in_flight:?}"
    );

    // Its pending properties are the LEAF properties it is applying, each named as the
    // configuration names it beside the value it is being set to.
    let pending: Vec<String> = in_flight["PendingProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name}: expected PendingProperties, got {in_flight:?}"))
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_owned())
        .collect();
    for expected in [
        "AIMLOptions.S3VectorsEngine : {\"Enabled\":false}",
        "CognitoOptions : {\"Enabled\":false}",
        "IPAddressType : ipv4",
        "SnapshotOptions : {\"AutomatedSnapshotStartHour\":0}",
    ] {
        assert!(
            pending.contains(&expected.to_string()),
            "case DescribeDomainChangeProgress({name:?}): expected the property {expected:?} \
             among {pending:?}"
        );
    }

    // And the same properties are what the domain reports as being modified, with the
    // value each is moving to and the kind that value is.
    let modifying = status["ModifyingProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name}: expected ModifyingProperties, got {status:?}"))
        .clone();
    for (property, pending_value, value_type) in [
        (
            "AIMLOptions.S3VectorsEngine",
            "{\"Enabled\":false}",
            "STRINGIFIED_JSON",
        ),
        ("IPAddressType", "ipv4", "PLAIN_TEXT"),
    ] {
        let entry = modifying
            .iter()
            .find(|entry| entry["Name"].as_str() == Some(property))
            .unwrap_or_else(|| {
                panic!("case {name}: expected {property} among {modifying:?}")
            });
        assert_eq!(
            (
                entry["PendingValue"].as_str(),
                entry["ValueType"].as_str(),
                entry["ActiveValue"].as_str(),
            ),
            (Some(pending_value), Some(value_type), Some("")),
            "case DescribeDomain({name:?}, {property}): expected the value it is moving to and \
             the kind that value is, got {entry:?}"
        );
    }

    // Once the cluster is up, nothing is being modified any more and a FURTHER change
    // is applied to a running cluster rather than waiting for one.
    tokio::time::sleep(SETTLE_WAIT).await;
    let settled = emulator.describe(name).await.body["DomainStatus"].clone();
    assert_eq!(
        settled["ModifyingProperties"].as_array().map(Vec::len),
        Some(0),
        "case DescribeDomain({name:?}, settled): a domain with no change in flight modifies \
         nothing, got {:?}",
        settled["ModifyingProperties"]
    );
    let updated = emulator
        .update_config(name, json!({ "SnapshotOptions": { "AutomatedSnapshotStartHour": 4 } }))
        .await;
    assert_eq!(
        updated.status, 200,
        "case UpdateDomainConfig({name:?}): expected 200, got {} with {:?}",
        updated.status, updated.body
    );
    assert_eq!(
        progress(&emulator, name).await["ConfigChangeStatus"].as_str(),
        Some("ApplyingChanges"),
        "case DescribeDomainChangeProgress({name:?}, update): a change on a provisioned cluster \
         is being applied"
    );
}

#[tokio::test]
async fn a_delete_applies_a_change_frees_the_name_and_is_not_found_twice() {
    let emulator = Emulator::new();
    // The rows span the two ways a name can hold no domain: one that was deleted and
    // one that was never created.
    for name in ["delete-once", "delete-twice"] {
        created(&emulator, name, json!({})).await;
    }

    let deleted = emulator.delete("delete-once").await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(delete-once): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        (
            deleted.body["DomainStatus"]["Deleted"].as_bool(),
            deleted.body["DomainStatus"]["DomainProcessingStatus"].as_str(),
            deleted.body["DomainStatus"]["ChangeProgressDetails"]["ConfigChangeStatus"].as_str(),
        ),
        (Some(true), Some("Deleting"), Some("ApplyingChanges")),
        "case DeleteDomain(delete-once): tearing the domain down is a change being applied to \
         it, got {:?}",
        deleted.body["DomainStatus"]
    );

    // The name is free again: a create of it is served rather than refused as taken.
    let again = emulator
        .create(json!({ "DomainName": "delete-once" }))
        .await;
    assert_eq!(
        again.status, 200,
        "case CreateDomain(delete-once, after delete): the name a delete released is free, got \
         {} with {:?}",
        again.status, again.body
    );

    // And a delete of a domain this account does not hold is not found, with the status
    // the service answers for a resource that is not there.
    for name in ["delete-twice", "never-created"] {
        if name == "delete-twice" {
            let first = emulator.delete(name).await;
            assert_eq!(
                first.status, 200,
                "case DeleteDomain({name:?}): expected 200, got {} with {:?}",
                first.status, first.body
            );
        }
        let answer = emulator.delete(name).await;
        assert_eq!(
            answer.status, 404,
            "case DeleteDomain({name:?}, absent): expected 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case DeleteDomain({name:?}, absent): expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn an_access_policy_is_held_in_the_service_rendering_of_the_document_it_denotes() {
    let emulator = Emulator::new();
    // The rows differ along the layout the SAME document was written in — spaces after
    // the separators, newlines and indentation, no whitespace at all — plus a document
    // with no policy in it and one that is not JSON at all. What the service holds is
    // the document, not the layout, so every layout of one document reads back the
    // same way.
    let document = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\"}]}";
    let rows: Vec<(&str, &str, &str)> = vec![
        ("policy-compact", document, document),
        (
            "policy-spaced",
            "{\"Version\": \"2012-10-17\", \"Statement\": [{\"Effect\": \"Allow\"}]}",
            document,
        ),
        (
            "policy-indented",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    { \"Effect\": \"Allow\" }\n  ]\n}",
            document,
        ),
        ("policy-empty-object", "{}", "{}"),
        // A domain created with no policy at all holds the empty document.
        ("policy-absent", "", ""),
    ];

    for (name, sent, expected) in rows {
        let mut body = json!({});
        if !sent.is_empty() {
            body["AccessPolicies"] = json!(sent);
        }
        let status = created(&emulator, name, body).await;
        let read_back = emulator.describe(name).await.body["DomainStatus"].clone();
        let config = configured(&emulator, name).await;
        for (reader, reported) in [
            ("CreateDomain", &status["AccessPolicies"]),
            ("DescribeDomain", &read_back["AccessPolicies"]),
            ("DescribeDomainConfig", &config["AccessPolicies"]["Options"]),
        ] {
            assert_eq!(
                reported.as_str(),
                Some(expected),
                "case {reader}({name:?}): expected the document {expected:?} the service holds \
                 for the policy sent as {sent:?}, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn health_reports_the_configured_cluster_and_only_observations_of_a_running_one() {
    let emulator = Emulator::new();
    // The rows span the topologies the configuration can describe: a single zone, two
    // and three zones, and a multi-AZ domain with a standby zone — each read while the
    // cluster is still being provisioned.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "health-single",
            json!({ "InstanceType": "r6g.large.search", "InstanceCount": 1 }),
            "1",
            "1",
        ),
        (
            "health-two-zones",
            json!({
                "InstanceType": "r6g.large.search",
                "InstanceCount": 2,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 2 },
            }),
            "2",
            "2",
        ),
        (
            "health-three-zones",
            json!({
                "InstanceType": "r6g.large.search",
                "InstanceCount": 3,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 3 },
            }),
            "3",
            "3",
        ),
        (
            "health-standby",
            json!({
                "InstanceType": "r6g.large.search",
                "InstanceCount": 3,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 3 },
                "MultiAZWithStandbyEnabled": true,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterType": "r6g.large.search",
                "DedicatedMasterCount": 3,
            }),
            "3",
            // One of the three zones is held in standby, so two serve traffic.
            "2",
        ),
    ];

    for (name, cluster, zones, active_zones) in rows {
        created(&emulator, name, json!({ "ClusterConfig": cluster.clone() })).await;
        let body = health(&emulator, name).await;
        assert_eq!(
            (
                body["AvailabilityZoneCount"].as_str(),
                body["ActiveAvailabilityZoneCount"].as_str(),
                body["MasterNode"].as_str(),
            ),
            (Some(zones), Some(active_zones), Some("Available")),
            "case DescribeDomainHealth({name:?}, {cluster}): the zones a domain is placed in and \
             the master elected from the nodes its configuration names are known from the \
             create, got {body:?}"
        );
        // The shard allocation is an OBSERVATION of a running cluster, and so is the
        // per-zone information: a domain whose cluster is not up yet reports neither.
        assert_eq!(
            (
                body["ClusterHealth"].as_str(),
                body["TotalShards"].as_str(),
                body["EnvironmentInformation"].as_array(),
            ),
            (Some("NotAvailable"), Some("NotAvailable"), None),
            "case DescribeDomainHealth({name:?}): expected no observation of a cluster that is \
             not up, got {body:?}"
        );
    }

    // Once the clock has provisioned them, the same read reports the running cluster,
    // zone by zone.
    tokio::time::sleep(SETTLE_WAIT).await;
    let body = health(&emulator, "health-three-zones").await;
    assert_eq!(
        (
            body["ClusterHealth"].as_str(),
            body["EnvironmentInformation"]
                .as_array()
                .and_then(|zones| zones.first())
                .and_then(|zone| zone["AvailabilityZoneInformation"].as_array())
                .map(Vec::len),
        ),
        (Some("Green"), Some(3)),
        "case DescribeDomainHealth(health-three-zones, settled): expected the running cluster's \
         three zones, got {body:?}"
    );
}

#[tokio::test]
async fn the_service_software_of_a_domain_being_provisioned_is_the_empty_version() {
    let emulator = Emulator::new();
    // Both sides of the provisioning span: a domain whose nodes do not exist yet runs
    // no service software, and one whose cluster is up runs the release the service
    // provisioned it with.
    let name = "software-version";
    let status = created(&emulator, name, json!({})).await;
    assert_eq!(
        (
            status["ServiceSoftwareOptions"]["CurrentVersion"].as_str(),
            status["ServiceSoftwareOptions"]["NewVersion"].as_str(),
            status["ServiceSoftwareOptions"]["AutomatedUpdateDate"].as_f64(),
        ),
        (Some(""), Some(""), Some(0.0)),
        "case CreateDomain({name:?}): a domain with no node yet runs no service software, has no \
         release to move onto and no automatic update due, got {:?}",
        status["ServiceSoftwareOptions"]
    );

    tokio::time::sleep(SETTLE_WAIT).await;
    let settled = emulator.describe(name).await.body["DomainStatus"].clone();
    let current = settled["ServiceSoftwareOptions"]["CurrentVersion"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        !current.is_empty(),
        "case DescribeDomain({name:?}, settled): a provisioned cluster runs a service software \
         release, got {:?}",
        settled["ServiceSoftwareOptions"]
    );
    assert_eq!(
        settled["ServiceSoftwareOptions"]["NewVersion"].as_str(),
        Some(""),
        "case DescribeDomain({name:?}, settled): a domain with no update available has no \
         release to move onto, got {:?}",
        settled["ServiceSoftwareOptions"]
    );
}

#[tokio::test]
async fn every_reader_of_a_domain_reports_the_one_stored_configuration() {
    let emulator = Emulator::new();
    // `DescribeDomains` reports the same `DomainStatus` as the single-domain reader,
    // for each of a SET of domains: the batch reader is a projection of the same
    // record, not a second one.
    let names = ["batch-one", "batch-two", "batch-three"];
    for name in names {
        create_baseline(&emulator, name).await;
    }

    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain-info"),
            Some(json!({ "DomainNames": names })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomains: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let listed = answer.body["DomainStatusList"]
        .as_array()
        .unwrap_or_else(|| panic!("case DescribeDomains: expected DomainStatusList, got {:?}", answer.body))
        .clone();
    assert_eq!(
        listed.len(),
        names.len(),
        "case DescribeDomains: expected one status per domain, got {listed:?}"
    );
    for (name, status) in names.iter().zip(listed.iter()) {
        let described = emulator.describe(name).await.body["DomainStatus"].clone();
        for (option, _) in server_defaults() {
            assert_eq!(
                status[option], described[option],
                "case DescribeDomains({name:?}, {option}): expected what DescribeDomain reports \
                 ({:?}), got {:?}",
                described[option], status[option]
            );
        }
    }
}
