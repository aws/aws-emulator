// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a DESCRIPTOR of a domain reports about it: the off-peak
//! window the service assigns, the properties a configuration change names, the address
//! and the service software a descriptor publishes, and the state each option is in.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness comes from [`super::domain_lifecycle`]; this suite adds no second
//! copy of it.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. A domain created without an off-peak window comes up with the window the SERVICE
//!    assigns — a start time of its own choosing, not the zero the shape's members
//!    default to — and one created with a window keeps the window it asked for. Every
//!    reader of the domain reports that one stored window.
//! 2. A configuration change names the properties it is applying at the grain the
//!    service applies them: the option blocks it applies PROPERTY BY PROPERTY are
//!    reported as `<Block>.<Property>` entries, one per property of the block, and every
//!    other option whole — each entry carrying the value the property is being moved to
//!    and `STRINGIFIED_JSON` where that value is JSON content, `PLAIN_TEXT` where it is a
//!    direct value. `DescribeDomainChangeProgress` names the same properties.
//! 3. `DomainStatus$Endpoint` is the address the control plane publishes for the domain's
//!    data plane, and a domain with no cluster behind it — one still being provisioned, or
//!    one being torn down — publishes no address and no service software version at all.
//! 4. `ServiceSoftwareOptions$CurrentVersion` is the release built for the engine version
//!    the domain runs.
//! 5. `DescribeDomainConfig` reports `Active` for an option a change in flight is
//!    applying nothing to, and the Auto-Tune settings the service fills in.
//! 6. `ChangeProgressStatus$TotalNumberOfStages` counts the stages that change reports.

use serde_json::{json, Value};

use super::change_progress::LEAF_PROPERTY_BLOCKS;
use super::domain_lifecycle::{settled_domain, Emulator, PREFIX};
use super::published_domain_address::emulator_authority;

/// The off-peak window start a domain that named none comes up with: the service assigns
/// every new domain "a custom 10-hour, low-traffic window during which OpenSearch Service
/// can perform mandatory configuration changes", and the start time it assigns is the
/// SERVICE's — not the zero `WindowStartTime`'s members are modelled to default to. Which
/// hour the real service picks is unpublished, so the emulator picks one of its own and
/// every reader of the domain reports that one.
const SERVICE_OFF_PEAK_START: (i64, i64) = (5, 0);

/// `PropertyValueType`'s two values, as `ModifyingProperties$ValueType` reports them
/// (`smithy_query(query="shape",shape="com.amazonaws.opensearch#PropertyValueType")`).
const STRINGIFIED_JSON: &str = "STRINGIFIED_JSON";
const PLAIN_TEXT: &str = "PLAIN_TEXT";

/// `CreateDomain` with `body`, asserting it was accepted, and the `DomainStatus` it
/// answered with.
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

/// The `DomainStatus` `DescribeDomain` reports for `name`.
async fn described(emulator: &Emulator, name: &str) -> Value {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
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

/// The `DomainStatus` of `name` once the provisioning the clock owes it has fallen due.
///
/// The harness's own poll ([`settled_domain`]) waits for the condition a client polls —
/// the domain reporting itself created with nothing in flight — rather than for a fixed
/// time, so no case here depends on how fast the host runs.
async fn settled(emulator: &Emulator, name: &str) -> Value {
    settled_domain(emulator, name).await
}

/// The entry `ModifyingProperties` carries for the property `name`, or `None` when it
/// carries none.
fn modifying<'a>(status: &'a Value, name: &str) -> Option<&'a Value> {
    status["ModifyingProperties"]
        .as_array()?
        .iter()
        .find(|entry| entry["Name"].as_str() == Some(name))
}

/// The property names `ModifyingProperties` carries, in the order it carries them.
fn modifying_names(status: &Value, case: &str) -> Vec<String> {
    status["ModifyingProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected ModifyingProperties, got {status:?}"))
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// RULE: a domain created without an off-peak window comes up with the window the SERVICE
/// assigns it, and one created with a window keeps the window it asked for — and every
/// reader of the domain reports that one stored window.
///
/// The rows differ along what the create says about the window: nothing at all, the
/// container with no window in it, a window at the bottom of `WindowStartTime$Hours`'
/// modelled `@range` (0..=23), one inside it and one at the top — so the service's own
/// default is told apart from a start a caller asked for, including the start that IS the
/// shape's modelled default.
#[tokio::test]
async fn a_domain_reports_the_off_peak_window_it_was_given_or_the_one_the_service_assigns() {
    let emulator = Emulator::new();
    let rows: Vec<(&str, Value, (i64, i64))> = vec![
        ("no off-peak options at all", json!({}), SERVICE_OFF_PEAK_START),
        (
            "the container with no window in it",
            json!({ "OffPeakWindowOptions": { "Enabled": true } }),
            SERVICE_OFF_PEAK_START,
        ),
        (
            "a window at the range's floor, which is also the shape's own default",
            json!({
                "OffPeakWindowOptions": {
                    "OffPeakWindow": { "WindowStartTime": { "Hours": 0, "Minutes": 0 } },
                },
            }),
            (0, 0),
        ),
        (
            "a window inside the range",
            json!({
                "OffPeakWindowOptions": {
                    "OffPeakWindow": { "WindowStartTime": { "Hours": 2, "Minutes": 30 } },
                },
            }),
            (2, 30),
        ),
        (
            "a window at the range's ceiling",
            json!({
                "OffPeakWindowOptions": {
                    "OffPeakWindow": { "WindowStartTime": { "Hours": 23, "Minutes": 59 } },
                },
            }),
            (23, 59),
        ),
    ];

    for (index, (case, body, (hours, minutes))) in rows.into_iter().enumerate() {
        let name = format!("offpeak-{index}");
        let expected = json!({ "Hours": hours, "Minutes": minutes });
        let created = created(&emulator, &name, body).await;
        let read_back = described(&emulator, &name).await;
        let config = configured(&emulator, &name).await;
        let deleted = emulator.delete(&name).await;
        assert_eq!(
            deleted.status, 200,
            "case DeleteDomain({case:?}): expected 200, got {} with {:?}",
            deleted.status, deleted.body
        );
        for (reader, window) in [
            ("CreateDomain", &created["OffPeakWindowOptions"]),
            ("DescribeDomain", &read_back["OffPeakWindowOptions"]),
            ("DescribeDomainConfig", &config["OffPeakWindowOptions"]["Options"]),
            (
                "DeleteDomain",
                &deleted.body["DomainStatus"]["OffPeakWindowOptions"],
            ),
        ] {
            assert_eq!(
                window["Enabled"].as_bool(),
                Some(true),
                "case {reader}({case:?}): every domain created now has an off-peak window, \
                 got {window:?}"
            );
            assert_eq!(
                window["OffPeakWindow"]["WindowStartTime"], expected,
                "case {reader}({case:?}): expected the window start {expected}, got {window:?}"
            );
        }
    }
}

/// RULE: the properties a configuration change reports are the properties the service
/// applies: the blocks it applies property by property are named `<Block>.<Property>`,
/// one entry per property of the block, every other option is named whole, and each entry
/// carries the value the property is moving to with `STRINGIFIED_JSON` for JSON content
/// and `PLAIN_TEXT` for a direct value. `DescribeDomainChangeProgress` names the same
/// properties.
///
/// The rows differ along the KIND of value a property carries — a nested container, a
/// scalar toggle, an instance type, a node count, a submitted JSON document and a
/// property the configuration does not use — so the naming and the classification are
/// each exercised on more than one shape of value.
#[tokio::test]
async fn a_change_names_the_leaf_properties_it_applies_and_the_kind_of_each_value() {
    let emulator = Emulator::new();
    let name = "leaf-properties";
    let policy = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\"Principal\":\
         {{\"AWS\":\"{}\"}},\"Action\":\"es:*\",\"Resource\":\"*\"}}]}}",
        ::engine::emulator_account_id()
    );
    let status = created(
        &emulator,
        name,
        json!({
            "ClusterConfig": {
                "InstanceType": "r6g.large.search",
                "InstanceCount": 2,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 2 },
            },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 20 },
            "AccessPolicies": policy.clone(),
            "TagList": [{ "Key": "team", "Value": "search" }],
        }),
    )
    .await;

    // Every property of an expanded block is named, with the dotted path the service
    // names it by, and the container around it is NOT named: it is the properties, not
    // the block, that the service applies.
    let names = modifying_names(&status, "CreateDomain(leaf-properties)");
    for (block, properties) in LEAF_PROPERTY_BLOCKS {
        assert!(
            !names.iter().any(|named| named == block),
            "case {block}: a block applied property by property is not named whole, got {names:?}"
        );
        for property in properties {
            let dotted = format!("{block}.{property}");
            assert!(
                names.contains(&dotted),
                "case {dotted}: expected the property among {names:?}"
            );
        }
    }
    // An option the service applies as a whole is named whole, so a caller reading one
    // reads the option's own document.
    for whole in ["AccessPolicies", "EBSOptions", "OffPeakWindowOptions", "SnapshotOptions"] {
        assert!(
            names.contains(&whole.to_string()),
            "case {whole}: expected the option named whole among {names:?}"
        );
    }

    // One row per kind of value: a nested container, two toggles, an instance type, a
    // node count, the submitted policy document, a whole option block and a property the
    // configuration does not use.
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "ClusterConfig.ColdStorageOptions",
            "{\"Enabled\":false}",
            STRINGIFIED_JSON,
        ),
        ("ClusterConfig.ZoneAwarenessEnabled", "true", PLAIN_TEXT),
        ("ClusterConfig.DedicatedMasterEnabled", "false", PLAIN_TEXT),
        ("ClusterConfig.InstanceType", "r6g.large.search", PLAIN_TEXT),
        ("ClusterConfig.InstanceCount", "2", PLAIN_TEXT),
        ("ClusterConfig.WarmType", "", PLAIN_TEXT),
        ("AdvancedSecurityOptions.AnonymousAuthEnabled", "false", PLAIN_TEXT),
        ("AccessPolicies", policy.as_str(), STRINGIFIED_JSON),
        ("IPAddressType", "ipv4", PLAIN_TEXT),
        (
            "SnapshotOptions",
            "{\"AutomatedSnapshotStartHour\":0}",
            STRINGIFIED_JSON,
        ),
        (
            "TAGS",
            "[{\"Key\":\"team\",\"Value\":\"search\"}]",
            STRINGIFIED_JSON,
        ),
    ];

    let pending: Vec<String> = progress(&emulator, name).await["PendingProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name}: expected PendingProperties"))
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_owned())
        .collect();

    for (property, value, value_type) in rows {
        let entry = modifying(&status, property).unwrap_or_else(|| {
            panic!("case {property}: expected the property among {names:?}")
        });
        assert_eq!(
            (
                entry["PendingValue"].as_str(),
                entry["ValueType"].as_str(),
                entry["ActiveValue"].as_str(),
            ),
            (Some(value), Some(value_type), Some("")),
            "case {property}: expected the value it is moving to, the kind that value is \
             and no value before it, got {entry:?}"
        );
        // The change reports the SAME property under the same name, as the
        // `<Name> : <value>` text its own list carries.
        let rendered = format!("{property} : {value}");
        assert!(
            pending.contains(&rendered),
            "case {property}: expected {rendered:?} among the pending properties {pending:?}"
        );
    }

    // And the same projection answers `DescribeDomainConfig`, so a caller that reads the
    // configuration sees the same properties in flight as one that reads the domain.
    let config = configured(&emulator, name).await;
    assert_eq!(
        modifying_names(&config, "DescribeDomainConfig(leaf-properties)"),
        names,
        "case DescribeDomainConfig(leaf-properties): expected the same properties the domain \
         reports"
    );

    // Once the cluster is up nothing is being modified any more.
    let settled = settled(&emulator, name).await;
    assert_eq!(
        settled["ModifyingProperties"].as_array().map(Vec::len),
        Some(0),
        "case DescribeDomain(leaf-properties, settled): a domain with no change in flight \
         modifies nothing, got {:?}",
        settled["ModifyingProperties"]
    );
}

/// RULE: a domain's public endpoint is the address the control plane PUBLISHES for its
/// data plane, the service software it reports is the release built for the engine version
/// it runs, and a domain with no cluster behind it — one still being provisioned, and one
/// being torn down — publishes neither.
///
/// The rows differ along the engine the domain runs, which is what the service-software
/// release is named for: two OpenSearch releases and a legacy Elasticsearch one, whose
/// releases are published under the bare release label.
#[tokio::test]
async fn a_domain_publishes_its_endpoint_host_and_the_software_its_engine_runs() {
    let emulator = Emulator::new();
    // (the case, the engine version, the service software release label it runs)
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "an OpenSearch engine",
            "OpenSearch_2.11",
            "OpenSearch_2_11_R20250101",
        ),
        (
            "another OpenSearch release",
            "OpenSearch_1.3",
            "OpenSearch_1_3_R20250101",
        ),
        (
            "a legacy Elasticsearch engine",
            "Elasticsearch_7.10",
            "R20250101",
        ),
    ];

    for (index, (case, engine_version, release)) in rows.into_iter().enumerate() {
        let name = format!("endpoint-{index}");
        // A domain whose cluster is not up yet has no address and no node running any
        // service software: both appear only once it is provisioned.
        let creating = created(
            &emulator,
            &name,
            json!({ "EngineVersion": engine_version }),
        )
        .await;
        assert_eq!(
            (
                creating["Endpoint"].as_str(),
                creating["ServiceSoftwareOptions"]["CurrentVersion"].as_str(),
            ),
            (None, Some("")),
            "case {case:?}: a domain still being provisioned publishes no endpoint and no \
             service software version, got {creating:?}"
        );

        let status = settled(&emulator, &name).await;
        let endpoint = status["Endpoint"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected an Endpoint, got {:?}", status["Endpoint"])
        });
        // The address a client of this emulator reaches the domain at: this emulator's own
        // endpoint, then the reserved path naming this service's data plane and the domain
        // behind it.
        assert_eq!(
            endpoint,
            format!("{}/_aws/es/{name}", emulator_authority()),
            "case {case:?}: expected the address this emulator serves the domain's data \
             plane at"
        );
        assert_eq!(
            status["ServiceSoftwareOptions"]["CurrentVersion"].as_str(),
            Some(release),
            "case {case:?}: expected the release built for {engine_version}, got {:?}",
            status["ServiceSoftwareOptions"]
        );
        // The address does not move between reads: a caller that stored it keeps it.
        assert_eq!(
            described(&emulator, &name).await["Endpoint"].as_str(),
            Some(endpoint),
            "case {case:?}: expected two reads to publish the same endpoint"
        );

        // A domain being torn down has no cluster any more: no address answers for it and
        // no node runs any service software.
        let deleted = emulator.delete(&name).await;
        assert_eq!(
            deleted.status, 200,
            "case DeleteDomain({case:?}): expected 200, got {} with {:?}",
            deleted.status, deleted.body
        );
        let torn_down = &deleted.body["DomainStatus"];
        assert_eq!(
            (
                torn_down["Deleted"].as_bool(),
                torn_down["Endpoint"].as_str(),
                torn_down["ServiceSoftwareOptions"]["CurrentVersion"].as_str(),
            ),
            (Some(true), None, Some("")),
            "case {case:?}: a domain being torn down publishes no endpoint and no service \
             software version, got {torn_down:?}"
        );
    }
}

/// RULE: `DescribeDomainConfig` reports every option's `OptionStatus`, `Processing` only
/// for an option the change in flight is applying something TO and `Active` for one it is
/// not — an option whose container is empty or whose every setting is switched off has
/// nothing to apply — and it carries the Auto-Tune settings the service fills in.
///
/// The rows differ along what the option holds: a feature the create configured, a
/// feature it switched off, a container it left empty, and the configuration of a domain
/// reached on a public endpoint, which has no VPC placement at all. The stage count is
/// asserted beside them, since it counts the stages the SAME change reports.
#[tokio::test]
async fn an_option_with_nothing_to_apply_is_active_while_the_rest_of_the_change_is_applied() {
    let emulator = Emulator::new();
    let name = "option-states";
    created(
        &emulator,
        name,
        json!({
            "ClusterConfig": { "InstanceType": "r6g.large.search", "InstanceCount": 2 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 20 },
            "EncryptionAtRestOptions": { "Enabled": false },
            "NodeToNodeEncryptionOptions": { "Enabled": true },
        }),
    )
    .await;
    let config = configured(&emulator, name).await;

    // (the case, the option, the state it is in while the create is being applied)
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "the cluster the create configured",
            "ClusterConfig",
            "Processing",
        ),
        (
            "a feature the create switched ON",
            "NodeToNodeEncryptionOptions",
            "Processing",
        ),
        (
            "the snapshot hour the service filled in, which is a setting at zero",
            "SnapshotOptions",
            "Processing",
        ),
        (
            "a feature the create switched OFF",
            "EncryptionAtRestOptions",
            "Active",
        ),
        (
            "a feature the create said nothing about",
            "CognitoOptions",
            "Active",
        ),
        (
            "a container the domain holds nothing in",
            "IdentityCenterOptions",
            "Active",
        ),
        (
            "the placement of a domain reached on a public endpoint",
            "VPCOptions",
            "Active",
        ),
        (
            "the log publishing of a domain that publishes no log type",
            "LogPublishingOptions",
            "Active",
        ),
    ];

    for (case, option, state) in rows {
        assert_eq!(
            config[option]["Status"]["State"].as_str(),
            Some(state),
            "case {case:?} ({option}): expected the option to be {state}, got {:?}",
            config[option]
        );
    }
    // The snapshot hour the service filled in is a setting in force even at zero: a
    // number says something is configured.
    assert_eq!(
        config["SnapshotOptions"]["Options"]["AutomatedSnapshotStartHour"].as_i64(),
        Some(0),
        "case the snapshot hour: expected the hour the service fills in, got {:?}",
        config["SnapshotOptions"]
    );

    // The Auto-Tune settings the service fills in are reported with the configuration:
    // "specify NO_ROLLBACK to retain all prior Auto-Tune settings", and the empty list of
    // maintenance schedules a domain with no schedule has.
    assert_eq!(
        (
            config["AutoTuneOptions"]["Options"]["RollbackOnDisable"].as_str(),
            config["AutoTuneOptions"]["Options"]["MaintenanceSchedules"].as_array(),
            config["AutoTuneOptions"]["Options"]["DesiredState"].as_str(),
        ),
        (Some("NO_ROLLBACK"), Some(&Vec::new()), Some("DISABLED")),
        "case AutoTuneOptions: expected the Auto-Tune settings the service fills in, got {:?}",
        config["AutoTuneOptions"]
    );

    // The stage count is the number of stages the change reports: a change that publishes
    // no stage reports none to count.
    let in_flight = progress(&emulator, name).await;
    assert_eq!(
        (
            in_flight["ChangeProgressStages"].as_array().map(Vec::len),
            in_flight["TotalNumberOfStages"].as_i64(),
        ),
        (Some(0), Some(0)),
        "case DescribeDomainChangeProgress({name:?}): expected the stage count to be the \
         number of stages reported, got {in_flight:?}"
    );

    // Once the change has been applied every option is Active, whatever it holds.
    settled(&emulator, name).await;
    let settled_config = configured(&emulator, name).await;
    for option in ["ClusterConfig", "NodeToNodeEncryptionOptions"] {
        assert_eq!(
            settled_config[option]["Status"]["State"].as_str(),
            Some("Active"),
            "case {option}: an option of a settled domain is Active, got {:?}",
            settled_config[option]
        );
    }
}
