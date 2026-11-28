// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the properties a configuration change is reported as applying:
//! `DomainStatus$ModifyingProperties`, `DomainConfig$ModifyingProperties`,
//! `ChangeProgressStatusDetails$PendingProperties` and the per-option `OptionStatus$State`
//! beside them.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness comes from [`super::domain_lifecycle`]; this suite adds no second
//! copy of it.
//!
//! The rule under test: the properties reported are drawn from the service's own
//! ENUMERATION of a domain's properties, in the order it enumerates them, one entry per
//! enumerated property the change in flight is applying and none for a part of the
//! configuration the service does not enumerate or the domain holds nothing in; each entry
//! carries the value that property is moving to, as the service encodes it — a security
//! sub-property as the toggle saying whether that feature is configured, the tag set as the
//! map of the tags the request carried, and a submitted document exactly as submitted; and
//! an option a property is reported for is `Processing` while the change is in flight,
//! every other option `Active`.
//!
//! The service has TWO such enumerations, one per surface: a DESCRIPTOR of the domain
//! reports what is being modified ([`enumerated_properties`]) and the CHANGE itself reports
//! what it is applying ([`change_enumerated_properties`]), which is the narrower list in
//! its own order.
//!
//! So the rows span a SET of creates: one that named nothing, three tag counts, a create
//! carrying a policy, a volume and a log type, and one with fine-grained access control
//! switched on.

use serde_json::{json, Value};

use super::domain_lifecycle::{Emulator, PREFIX};

/// `PropertyValueType`'s two values, as `ModifyingProperties$ValueType` reports them
/// (`smithy_query(query="shape",shape="com.amazonaws.opensearch#PropertyValueType")`).
const STRINGIFIED_JSON: &str = "STRINGIFIED_JSON";
const PLAIN_TEXT: &str = "PLAIN_TEXT";

/// `OptionState`'s two values a settling domain reports
/// (`smithy_query(query="shape",shape="com.amazonaws.opensearch#OptionState")`):
/// "Processing - The requested change is still in progress" and "Active - The requested
/// change is processed and deployed".
const PROCESSING: &str = "Processing";
const ACTIVE: &str = "Active";

/// The master user password the fine-grained-access-control row sends, within
/// `MasterUserOptions$MasterUserPassword`'s modelled length (8..128).
const MASTER_PASSWORD: &str = "Master-pass-1!";

/// An access policy naming the account as the bare account id a caller writes it as —
/// the form the document has to come back in, since the service holds the document the
/// caller submitted rather than one it re-authored.
///
/// This suite owns that document: a sibling suite that needs the same submitted policy
/// reaches it here rather than growing a second copy of it.
pub(crate) fn submitted_policy() -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\"Principal\":\
         {{\"AWS\":\"{}\"}},\"Action\":\"es:*\",\"Resource\":\"*\"}}]}}",
        ::engine::emulator_account_id()
    )
}

/// One create to make: the case it stands for and the body to send, less the domain name.
///
/// The rows differ along what the create gives the domain: nothing at all, one tag, three
/// tags, a submitted policy beside a volume and a log type, and fine-grained access
/// control with an internal user database — so the enumeration, the per-property encoding
/// and the per-option state are each exercised on more than one configuration.
fn rows() -> Vec<(&'static str, Value)> {
    vec![
        ("a create that named nothing", json!({})),
        (
            "a create with one tag",
            json!({ "TagList": [{ "Key": "Owner", "Value": "teamA" }] }),
        ),
        (
            "a create with three tags",
            json!({
                "TagList": [
                    { "Key": "Owner", "Value": "teamA" },
                    { "Key": "cost-centre", "Value": "42" },
                    { "Key": "Env", "Value": "staging" },
                ],
            }),
        ),
        (
            "a create with a policy, a volume, a log type and two features switched off",
            json!({
                "AccessPolicies": submitted_policy(),
                "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
                "LogPublishingOptions": { "SEARCH_SLOW_LOGS": { "Enabled": false } },
                "EncryptionAtRestOptions": { "Enabled": false },
                "AdvancedSecurityOptions": { "Enabled": false },
            }),
        ),
        (
            "a create with fine-grained access control on",
            json!({
                "EncryptionAtRestOptions": { "Enabled": true },
                "NodeToNodeEncryptionOptions": { "Enabled": true },
                "DomainEndpointOptions": { "EnforceHTTPS": true },
                "AdvancedSecurityOptions": {
                    "Enabled": true,
                    "InternalUserDatabaseEnabled": true,
                    "MasterUserOptions": {
                        "MasterUserName": "master",
                        "MasterUserPassword": MASTER_PASSWORD,
                    },
                },
            }),
        ),
    ]
}

/// The properties of a domain's configuration a CHANGE enumerates when it reports what it
/// is applying — `ChangeProgressStatusDetails$PendingProperties` — in the order it
/// enumerates them, stated here independently of the implementation.
///
/// Recorded behaviour is that this is NOT the list a descriptor of the domain reports as
/// being modified ([`enumerated_properties`]): a change reports no property for the
/// advanced options map, names the security block by four of its properties rather than
/// seven, and enumerates the cluster's own topology after the endpoint options and the EBS
/// volume rather than before them.
///
/// Those three differences, and every position up to and including the cluster's cold
/// storage, are what the recorded response discloses. The order of the properties AFTER
/// the cluster's topology is pinned here as a determinism and positional-stability check —
/// two reads of one change must report its properties in the same order — rather than as
/// recorded behaviour: a recording that discloses the service's own order for them should
/// be followed instead of this row.
fn change_enumerated_properties(body: &Value) -> Vec<String> {
    let mut names: Vec<&str> = vec![
        "AIMLOptions.NaturalLanguageQueryGenerationOptions",
        "AIMLOptions.S3VectorsEngine",
        "AIMLOptions.ServerlessVectorAcceleration",
    ];
    if body["AccessPolicies"]
        .as_str()
        .is_some_and(|policy| !policy.is_empty())
    {
        names.push("AccessPolicies");
    }
    names.extend([
        "AdvancedSecurityOptions.AnonymousAuthEnabled",
        "AdvancedSecurityOptions.InternalUserDatabaseEnabled",
        "AdvancedSecurityOptions.MasterUserOptions",
        "AdvancedSecurityOptions.SAMLOptions",
        "DomainEndpointOptions",
    ]);
    if body.get("EBSOptions").is_some() {
        names.push("EBSOptions");
    }
    names.extend([
        "ClusterConfig.ColdStorageOptions",
        "ClusterConfig.DedicatedMasterCount",
        "ClusterConfig.DedicatedMasterEnabled",
        "ClusterConfig.DedicatedMasterType",
        "ClusterConfig.InstanceCount",
        "ClusterConfig.InstanceType",
        "ClusterConfig.MultiAZWithStandbyEnabled",
        "ClusterConfig.WarmCount",
        "ClusterConfig.WarmEnabled",
        "ClusterConfig.WarmStorage",
        "ClusterConfig.WarmType",
        "ClusterConfig.ZoneAwarenessEnabled",
        "TAGS",
        "AutomatedSnapshotPauseOptions",
        "DeploymentStrategyOptions",
        "EncryptionAtRestOptions",
        "EngineVersion",
        "IPAddressType",
    ]);
    if body["LogPublishingOptions"]
        .as_object()
        .is_some_and(|types| !types.is_empty())
    {
        names.push("LogPublishingOptions");
    }
    names.extend([
        "NodeToNodeEncryptionOptions",
        "OffPeakWindowOptions",
        "SnapshotOptions",
        "SoftwareUpdateOptions",
    ]);
    names.into_iter().map(str::to_owned).collect()
}

/// The properties of a domain's configuration the service enumerates when it reports a
/// change, in the order it enumerates them — stated here independently of the
/// implementation, so a change in which properties the service reports, or in what order,
/// is a change this suite sees.
///
/// A block the service applies property by property stands for its own properties, named
/// `<Block>.<Property>`; every other enumerated option is named whole. A property the
/// domain holds NOTHING for is no property at all: the access policy of a domain created
/// without one, the volume of a domain with no EBS storage, the log publishing of a domain
/// that publishes no log type, the Identity Center integration of a domain that names no
/// instance and the placement of a domain reached on a public endpoint are each left out.
fn enumerated_properties(body: &Value) -> Vec<String> {
    let mut names: Vec<&str> = vec![
        "AIMLOptions.NaturalLanguageQueryGenerationOptions",
        "AIMLOptions.S3VectorsEngine",
        "AIMLOptions.ServerlessVectorAcceleration",
    ];
    if body["AccessPolicies"]
        .as_str()
        .is_some_and(|policy| !policy.is_empty())
    {
        names.push("AccessPolicies");
    }
    names.extend([
        "AdvancedOptions",
        "AdvancedSecurityOptions.AnonymousAuthDisableDate",
        "AdvancedSecurityOptions.AnonymousAuthEnabled",
        "AdvancedSecurityOptions.IAMFederationOptions",
        "AdvancedSecurityOptions.InternalUserDatabaseEnabled",
        "AdvancedSecurityOptions.JWTOptions",
        "AdvancedSecurityOptions.MasterUserOptions",
        "AdvancedSecurityOptions.SAMLOptions",
        "ClusterConfig.ColdStorageOptions",
        "ClusterConfig.DedicatedMasterCount",
        "ClusterConfig.DedicatedMasterEnabled",
        "ClusterConfig.DedicatedMasterType",
        "ClusterConfig.InstanceCount",
        "ClusterConfig.InstanceType",
        "ClusterConfig.MultiAZWithStandbyEnabled",
        "ClusterConfig.WarmCount",
        "ClusterConfig.WarmEnabled",
        "ClusterConfig.WarmStorage",
        "ClusterConfig.WarmType",
        "ClusterConfig.ZoneAwarenessEnabled",
        "DomainEndpointOptions",
    ]);
    if body.get("EBSOptions").is_some() {
        names.push("EBSOptions");
    }
    names.extend([
        "TAGS",
        "AutomatedSnapshotPauseOptions",
        "DeploymentStrategyOptions",
        "EncryptionAtRestOptions",
        "EngineVersion",
        "IPAddressType",
    ]);
    if body["LogPublishingOptions"]
        .as_object()
        .is_some_and(|types| !types.is_empty())
    {
        names.push("LogPublishingOptions");
    }
    names.extend([
        "NodeToNodeEncryptionOptions",
        "OffPeakWindowOptions",
        "SnapshotOptions",
        "SoftwareUpdateOptions",
    ]);
    names.into_iter().map(str::to_owned).collect()
}

/// The parts of a domain's configuration the service applies WITHOUT reporting a property
/// for them.
///
/// Recorded behaviour: a domain whose create is still being applied reports none of these
/// among the properties being modified.
const UNENUMERATED_OPTIONS: [&str; 6] = [
    "AutoTuneOptions",
    "CognitoOptions",
    "EngineMode",
    "UseCase",
    "IdentityCenterOptions",
    "VPCOptions",
];

/// Those of them whose container carries an `OptionStatus`, so there is an `OptionState`
/// to read, and which a domain nothing is configured in for them is applying nothing for —
/// so each is `Active` while the rest of the configuration settles.
///
/// The Auto-Tune container reports `AutoTuneStatus` instead, whose state is one of
/// `AutoTuneState`'s values rather than an option state; the engine mode and the use case
/// are settings the service gives every domain a value for, which it applies with the rest
/// of the configuration — so none of the three is in this table.
const UNENUMERATED_WITH_OPTION_STATUS: [&str; 3] =
    ["CognitoOptions", "IdentityCenterOptions", "VPCOptions"];

/// `CreateDomain` with `body` under `name`, asserting it was accepted, and the
/// `DomainStatus` it answered with.
async fn created(emulator: &Emulator, name: &str, body: &Value) -> Value {
    let mut request = body.clone();
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

/// The `PendingProperties` `DescribeDomainChangeProgress` reports for the change in flight
/// on `name`, as the `<Name> : <value>` texts the list carries.
async fn pending_properties(emulator: &Emulator, name: &str) -> Vec<String> {
    let answer = emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}/progress"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomainChangeProgress({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["ChangeProgressStatus"]["PendingProperties"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case DescribeDomainChangeProgress({name:?}): expected PendingProperties, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The property names a `ModifyingProperties` list carries, in the order it carries them.
fn reported_names(reported: &Value, case: &str) -> Vec<String> {
    reported["ModifyingProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected ModifyingProperties, got {reported:?}"))
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The entry a `ModifyingProperties` list carries for the property `name`.
fn reported_entry(reported: &Value, name: &str, case: &str) -> Value {
    reported["ModifyingProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected ModifyingProperties, got {reported:?}"))
        .iter()
        .find(|entry| entry["Name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("case {case}: expected a {name} entry, got {reported:?}"))
        .clone()
}

/// The `(PendingValue, ValueType)` pair an entry carries.
fn reported_value(reported: &Value, name: &str, case: &str) -> (String, String) {
    let entry = reported_entry(reported, name, case);
    (
        entry["PendingValue"].as_str().unwrap_or_default().to_owned(),
        entry["ValueType"].as_str().unwrap_or_default().to_owned(),
    )
}

/// The names in a `PendingProperties` list, taken off the `<Name> : <value>` texts.
fn pending_names(properties: &[String]) -> Vec<String> {
    properties
        .iter()
        .map(|property| {
            property
                .split_once(" : ")
                .map_or(property.clone(), |(name, _)| name.to_owned())
        })
        .collect()
}

#[tokio::test]
async fn every_reader_enumerates_the_same_properties_in_the_same_order() {
    let emulator = Emulator::new();
    for (index, (case, body)) in rows().into_iter().enumerate() {
        let name = format!("props-{index}");
        let expected = enumerated_properties(&body);
        let created = created(&emulator, &name, &body).await;
        assert_eq!(
            reported_names(&created, case),
            expected,
            "case {case:?} (CreateDomain): expected the enumerated properties of the change in \
             flight, got {:?}",
            created["ModifyingProperties"]
        );
        // The projection is of the STORED domain, so every reader of it reports the same
        // list in the same order — a caller indexing one list and a caller indexing
        // another read the same property.
        let described = described(&emulator, &name).await;
        assert_eq!(
            reported_names(&described, case),
            expected,
            "case {case:?} (DescribeDomain): expected the same properties, got {:?}",
            described["ModifyingProperties"]
        );
        let config = configured(&emulator, &name).await;
        assert_eq!(
            reported_names(&config, case),
            expected,
            "case {case:?} (DescribeDomainConfig): expected the same properties, got {:?}",
            config["ModifyingProperties"]
        );
        let pending = pending_properties(&emulator, &name).await;
        assert_eq!(
            pending_names(&pending),
            change_enumerated_properties(&body),
            "case {case:?} (DescribeDomainChangeProgress): the change names its own \
             properties, got {pending:?}"
        );
    }
}

#[tokio::test]
async fn each_property_carries_the_value_it_is_moving_to_as_the_service_encodes_it() {
    let emulator = Emulator::new();
    for (index, (case, body)) in rows().into_iter().enumerate() {
        let name = format!("values-{index}");
        let created = created(&emulator, &name, &body).await;

        // The tag set is the MAP of the tags the request carried, so a caller confirming
        // tag-on-create reads its own tags by key; a create that carried none leaves the
        // property with no value at all, which is no JSON document to parse.
        let tags: Value = body["TagList"]
            .as_array()
            .map(|list| {
                Value::Object(
                    list.iter()
                        .map(|tag| {
                            (
                                tag["Key"].as_str().unwrap_or_default().to_owned(),
                                tag["Value"].clone(),
                            )
                        })
                        .collect(),
                )
            })
            .unwrap_or(Value::Null);
        let (pending_value, value_type) = reported_value(&created, "TAGS", case);
        let expected_type = if tags.is_null() {
            PLAIN_TEXT
        } else {
            STRINGIFIED_JSON
        };
        assert_eq!(
            value_type, expected_type,
            "case {case:?} (TAGS): expected the kind {expected_type}, got {pending_value:?} as \
             {value_type}"
        );
        if tags.is_null() {
            assert_eq!(
                pending_value, "",
                "case {case:?} (TAGS): a create that carried no tag leaves the property empty, \
                 got {pending_value:?}"
            );
        } else {
            assert_eq!(
                ::serde_json::from_str::<Value>(&pending_value).ok(),
                Some(tags.clone()),
                "case {case:?} (TAGS): expected the map of the tags the request carried \
                 ({tags}), got {pending_value:?}"
            );
        }

        // A security sub-property is a TOGGLE: the block says, property by property,
        // whether each feature is configured, so one the domain holds nothing for is
        // reported off rather than with no value — never an empty marker a caller would
        // have to read as "unknown".
        let configured_internal_database = body["AdvancedSecurityOptions"]
            ["InternalUserDatabaseEnabled"]
            .as_bool()
            .unwrap_or(false);
        for (property, expected) in [
            ("AdvancedSecurityOptions.AnonymousAuthDisableDate", "false"),
            ("AdvancedSecurityOptions.AnonymousAuthEnabled", "false"),
            ("AdvancedSecurityOptions.IAMFederationOptions", "false"),
            (
                "AdvancedSecurityOptions.InternalUserDatabaseEnabled",
                if configured_internal_database {
                    "true"
                } else {
                    "false"
                },
            ),
            ("AdvancedSecurityOptions.JWTOptions", "false"),
            ("AdvancedSecurityOptions.SAMLOptions", "false"),
        ] {
            assert_eq!(
                reported_value(&created, property, case),
                (expected.to_owned(), PLAIN_TEXT.to_owned()),
                "case {case:?} ({property}): expected the toggle {expected:?} as plain text, got \
                 {:?}",
                reported_entry(&created, property, case)
            );
        }

        // An option the service applies whole carries its own document — the pause
        // CONFIGURATION, and not the derived state of a pause nothing has started.
        assert_eq!(
            reported_value(&created, "AutomatedSnapshotPauseOptions", case),
            ("{\"Enabled\":false}".to_owned(), STRINGIFIED_JSON.to_owned()),
            "case {case:?} (AutomatedSnapshotPauseOptions): expected the pause configuration, \
             got {:?}",
            reported_entry(&created, "AutomatedSnapshotPauseOptions", case)
        );

        // A property of a block the configuration does not use has no value, which is the
        // empty text and no JSON document.
        assert_eq!(
            reported_value(&created, "ClusterConfig.DedicatedMasterCount", case),
            (String::new(), PLAIN_TEXT.to_owned()),
            "case {case:?} (ClusterConfig.DedicatedMasterCount): a cluster with no dedicated \
             master has no count, got {:?}",
            reported_entry(&created, "ClusterConfig.DedicatedMasterCount", case)
        );

        // And a document the caller submitted is the value the property is moving to,
        // exactly as submitted: a principal written as the bare account id stays the bare
        // account id.
        if let Some(policy) = body["AccessPolicies"].as_str() {
            let (pending_value, value_type) = reported_value(&created, "AccessPolicies", case);
            assert_eq!(
                (
                    ::serde_json::from_str::<Value>(&pending_value).ok(),
                    value_type.as_str()
                ),
                (
                    ::serde_json::from_str::<Value>(policy).ok(),
                    STRINGIFIED_JSON
                ),
                "case {case:?} (AccessPolicies): expected the submitted document, got \
                 {pending_value:?}"
            );
            assert!(
                pending_value.contains(&format!("\"AWS\":\"{}\"", ::engine::emulator_account_id())),
                "case {case:?} (AccessPolicies): expected the principal as submitted, got \
                 {pending_value:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_option_no_property_is_reported_for_is_active_while_the_change_is_applied() {
    let emulator = Emulator::new();
    for (index, (case, body)) in rows().into_iter().enumerate() {
        let name = format!("states-{index}");
        let created = created(&emulator, &name, &body).await;
        let names = reported_names(&created, case);
        let config = configured(&emulator, &name).await;

        // A part of the configuration the service does not enumerate is applied without a
        // property being reported for it, and the container it is reported in — where the
        // domain has one at all — stays Active while the rest of the configuration
        // settles.
        for option in UNENUMERATED_OPTIONS {
            assert!(
                !names.iter().any(|named| named == option),
                "case {case:?} ({option}): expected no property to be reported for it, got \
                 {names:?}"
            );
        }
        for option in UNENUMERATED_WITH_OPTION_STATUS {
            let state = config[option]["Status"]["State"].as_str();
            assert!(
                state.is_none() || state == Some(ACTIVE),
                "case {case:?} ({option}): expected {ACTIVE} or no container, got {:?}",
                config[option]
            );
        }

        // An option the service reports a property for is one it is APPLYING, whatever
        // that property's value says: a feature switched OFF is a configuration the
        // service applies just as one switched on is, so the option is `Processing` until
        // the change lands.
        for option in [
            "ClusterConfig",
            "AdvancedSecurityOptions",
            "EncryptionAtRestOptions",
            "NodeToNodeEncryptionOptions",
            "DomainEndpointOptions",
            "SnapshotOptions",
        ] {
            assert_eq!(
                config[option]["Status"]["State"].as_str(),
                Some(PROCESSING),
                "case {case:?} ({option}): an option the change is applying is {PROCESSING}, got \
                 {:?}",
                config[option]
            );
        }

        // And so is a setting the service gives every domain a value for but reports no
        // property for: the engine mode and the use case of a domain are applied with the
        // rest of its configuration.
        for option in ["EngineMode", "UseCase"] {
            assert_eq!(
                config[option]["Status"]["State"].as_str(),
                Some(PROCESSING),
                "case {case:?} ({option}): a setting the service applies is {PROCESSING}, got \
                 {:?}",
                config[option]
            );
        }

        // The log publishing of a domain that publishes no log type is the container case
        // the other way round: the domain holds nothing in it, so there is nothing to
        // apply and nothing to report.
        if body["LogPublishingOptions"].is_null() {
            assert_eq!(
                config["LogPublishingOptions"]["Status"]["State"].as_str(),
                Some(ACTIVE),
                "case {case:?} (LogPublishingOptions): a domain that publishes no log type has \
                 nothing to apply, got {:?}",
                config["LogPublishingOptions"]
            );
        }
    }
}
