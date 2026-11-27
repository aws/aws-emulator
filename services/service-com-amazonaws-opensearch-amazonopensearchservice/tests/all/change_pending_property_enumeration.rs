// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the properties a CHANGE reports as its own:
//! `ChangeProgressStatusDetails$PendingProperties` while the change is in flight and
//! `$CompletedProperties` once the clock has made it due.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness comes from [`super::domain_lifecycle`]; this suite adds no second copy
//! of it.
//!
//! The rule under test: a change reports its own properties by the enumeration the service
//! uses for a CHANGE, which is NARROWER than the one a descriptor of the domain uses for
//! `ModifyingProperties` and in its own order (recorded behaviour) —
//!
//! * a change reports no property for the advanced options map, nor for the security
//!   block's anonymous-auth date, IAM-federation and JWT settings, each of which a
//!   descriptor of the domain does report as a property being modified;
//! * it enumerates the cluster's own topology AFTER the endpoint options and the EBS
//!   volume, where a descriptor enumerates it before them; and
//! * every property it does name carries the same value text the descriptor reports for
//!   that property, as `<Name> : <value>`, and the list a change reports as pending while
//!   it is in flight is the list it reports as completed once it has landed.
//!
//! The rows span a SET of creates — one that named nothing, one that set the advanced
//! options map itself, one with a policy, a volume and a log type, one with fine-grained
//! access control and a tag, and one with a dedicated master and zone awareness — because
//! the rule is about which properties the surface enumerates, whatever the request named.

use serde_json::{json, Value};

use super::domain_lifecycle::{
    log_group_arn, settled_domain, Emulator, CHANGE_LOG_GROUP, PREFIX,
};

/// The properties a DESCRIPTOR of the domain reports as being modified but a CHANGE
/// reports no property for at all (recorded behaviour): the advanced options map and three
/// of the security block's settings.
const REPORTED_ONLY_BY_A_DESCRIPTOR: [&str; 4] = [
    "AdvancedOptions",
    "AdvancedSecurityOptions.AnonymousAuthDisableDate",
    "AdvancedSecurityOptions.IAMFederationOptions",
    "AdvancedSecurityOptions.JWTOptions",
];

/// The master user password the fine-grained-access-control row sends, within
/// `MasterUserOptions$MasterUserPassword`'s modelled length (8..128).
const MASTER_PASSWORD: &str = "Master-pass-1!";

/// An access policy naming the account as the bare account id a caller writes it as.
fn submitted_policy() -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\"Principal\":\
         {{\"AWS\":\"{}\"}},\"Action\":\"es:*\",\"Resource\":\"*\"}}]}}",
        ::engine::emulator_account_id()
    )
}

/// One create to make: the case it stands for and the body to send, less the domain name.
fn rows() -> Vec<(&'static str, Value)> {
    vec![
        ("a create that named nothing", json!({})),
        (
            "a create that set the advanced options map itself",
            json!({
                "AdvancedOptions": {
                    "rest.action.multi.allow_explicit_index": "false",
                    "indices.fielddata.cache.size": "40",
                },
            }),
        ),
        (
            "a create with a policy, a volume and a log type",
            json!({
                "AccessPolicies": submitted_policy(),
                "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
                "LogPublishingOptions": {
                    "INDEX_SLOW_LOGS": {
                        "Enabled": true,
                        "CloudWatchLogsLogGroupArn": log_group_arn(CHANGE_LOG_GROUP),
                    },
                },
            }),
        ),
        (
            "a create with fine-grained access control and a tag",
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
                "TagList": [{ "Key": "Owner", "Value": "teamA" }],
            }),
        ),
        (
            "a create with a dedicated master and zone awareness",
            json!({
                "ClusterConfig": {
                    "InstanceType": "r6g.large.search",
                    "InstanceCount": 2,
                    "DedicatedMasterEnabled": true,
                    "DedicatedMasterType": "r6g.large.search",
                    "DedicatedMasterCount": 3,
                    "ZoneAwarenessEnabled": true,
                    "ZoneAwarenessConfig": { "AvailabilityZoneCount": 2 },
                },
                "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 20 },
            }),
        ),
    ]
}

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

/// The `ChangeProgressStatus` `DescribeDomainChangeProgress` reports for the change in
/// flight on `name`.
async fn progress(emulator: &Emulator, name: &str) -> Value {
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
    answer.body["ChangeProgressStatus"].clone()
}

/// The `<Name> : <value>` entries a property list of a change carries.
fn entries(progress: &Value, member: &str, case: &str) -> Vec<String> {
    progress[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a list {member}, got {progress:?}"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected strings in {member}, got {progress:?}")
                })
                .to_owned()
        })
        .collect()
}

/// The names those entries carry, in the order they carry them.
fn names(entries: &[String]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry
                .split_once(" : ")
                .map_or(entry.clone(), |(name, _)| name.to_owned())
        })
        .collect()
}

/// The `PendingValue` a `ModifyingProperties` list carries for the property `name`, or
/// `None` where it names no such property.
fn modifying_value(status: &Value, name: &str, case: &str) -> Option<String> {
    status["ModifyingProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected ModifyingProperties, got {status:?}"))
        .iter()
        .find(|entry| entry["Name"].as_str() == Some(name))
        .map(|entry| entry["PendingValue"].as_str().unwrap_or_default().to_owned())
}

/// The position of the FIRST property of the block `block` in `names`, or `None` where the
/// list names none of its properties.
fn first_of(names: &[String], block: &str) -> Option<usize> {
    let prefix = format!("{block}.");
    names
        .iter()
        .position(|name| name == block || name.starts_with(&prefix))
}

/// The position of the LAST property of the block `block` in `names`.
fn last_of(names: &[String], block: &str) -> Option<usize> {
    let prefix = format!("{block}.");
    names
        .iter()
        .rposition(|name| name == block || name.starts_with(&prefix))
}

#[tokio::test]
async fn a_change_reports_none_of_the_properties_only_a_descriptor_enumerates() {
    let emulator = Emulator::with_log_groups();
    emulator.created_log_group(CHANGE_LOG_GROUP).await;
    for (index, (case, body)) in rows().into_iter().enumerate() {
        let name = format!("change-props-{index}");
        let status = created(&emulator, &name, &body).await;
        let in_flight = progress(&emulator, &name).await;
        let pending = entries(&in_flight, "PendingProperties", case);
        let pending_names = names(&pending);

        for property in REPORTED_ONLY_BY_A_DESCRIPTOR {
            assert!(
                !pending_names.iter().any(|named| named == property),
                "case {case:?} ({property}): a change reports no property for it, got \
                 {pending_names:?}"
            );
            // And the descriptor of the same domain does report it, so the two surfaces
            // differ exactly here rather than the property having been dropped everywhere.
            assert!(
                modifying_value(&status, property, case).is_some(),
                "case {case:?} ({property}): a descriptor still reports it as being \
                 modified, got {:?}",
                status["ModifyingProperties"]
            );
        }
    }
}

#[tokio::test]
async fn every_property_a_change_names_carries_the_value_the_descriptor_reports() {
    let emulator = Emulator::with_log_groups();
    emulator.created_log_group(CHANGE_LOG_GROUP).await;
    for (index, (case, body)) in rows().into_iter().enumerate() {
        let name = format!("change-values-{index}");
        let status = created(&emulator, &name, &body).await;
        let in_flight = progress(&emulator, &name).await;
        let pending = entries(&in_flight, "PendingProperties", case);
        assert!(
            !pending.is_empty(),
            "case {case:?}: the change a create starts has properties pending, got \
             {in_flight:?}"
        );

        for entry in &pending {
            let (property, value) = entry.split_once(" : ").unwrap_or_else(|| {
                panic!("case {case:?}: expected a `<Name> : <value>` entry, got {entry:?}")
            });
            assert_eq!(
                modifying_value(&status, property, case).as_deref(),
                Some(value),
                "case {case:?} ({property}): expected the value the descriptor reports for \
                 it, got {entry:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_change_enumerates_the_cluster_after_the_endpoint_options_and_the_volume() {
    let emulator = Emulator::with_log_groups();
    emulator.created_log_group(CHANGE_LOG_GROUP).await;
    for (index, (case, body)) in rows().into_iter().enumerate() {
        let name = format!("change-order-{index}");
        let status = created(&emulator, &name, &body).await;
        let in_flight = progress(&emulator, &name).await;
        let pending_names = names(&entries(&in_flight, "PendingProperties", case));
        let modifying_names: Vec<String> = status["ModifyingProperties"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected ModifyingProperties, got {status:?}"))
            .iter()
            .map(|entry| entry["Name"].as_str().unwrap_or_default().to_owned())
            .collect();

        // On the change's own surface the endpoint options and the volume come first and
        // the cluster's topology after them; on the descriptor's the cluster comes before
        // both. Each list carries every block, so every position below is a real one.
        let cluster = first_of(&pending_names, "ClusterConfig")
            .unwrap_or_else(|| panic!("case {case:?}: expected cluster properties, got {pending_names:?}"));
        let endpoint = first_of(&pending_names, "DomainEndpointOptions")
            .unwrap_or_else(|| panic!("case {case:?}: expected DomainEndpointOptions, got {pending_names:?}"));
        assert!(
            endpoint < cluster,
            "case {case:?}: a change enumerates the endpoint options before the cluster, got \
             {pending_names:?}"
        );
        if let Some(volume) = first_of(&pending_names, "EBSOptions") {
            assert!(
                volume < cluster,
                "case {case:?}: a change enumerates the volume before the cluster, got \
                 {pending_names:?}"
            );
        }
        let security = last_of(&pending_names, "AdvancedSecurityOptions").unwrap_or_else(|| {
            panic!("case {case:?}: expected security properties, got {pending_names:?}")
        });
        assert!(
            security < endpoint,
            "case {case:?}: a change enumerates the security block before the endpoint \
             options, got {pending_names:?}"
        );
        if let Some(policy) = first_of(&pending_names, "AccessPolicies") {
            assert!(
                policy < security,
                "case {case:?}: a change enumerates the access policy before the security \
                 block, got {pending_names:?}"
            );
        }

        let descriptor_cluster = first_of(&modifying_names, "ClusterConfig")
            .unwrap_or_else(|| panic!("case {case:?}: expected cluster properties, got {modifying_names:?}"));
        let descriptor_endpoint = first_of(&modifying_names, "DomainEndpointOptions")
            .unwrap_or_else(|| panic!("case {case:?}: expected DomainEndpointOptions, got {modifying_names:?}"));
        assert!(
            descriptor_cluster < descriptor_endpoint,
            "case {case:?}: a descriptor enumerates the cluster before the endpoint options, \
             got {modifying_names:?}"
        );
    }
}

#[tokio::test]
async fn the_properties_a_change_reported_as_pending_are_the_ones_it_reports_completed() {
    let emulator = Emulator::new();
    for (index, (case, body)) in rows().into_iter().enumerate() {
        // The log-publishing row's destination is not mounted in this case's emulator, so
        // the body is sent without it: what is under test is the list, not the log type.
        let mut body = body.clone();
        if let Some(members) = body.as_object_mut() {
            members.remove("LogPublishingOptions");
        }
        let name = format!("change-settled-{index}");
        created(&emulator, &name, &body).await;
        let in_flight = progress(&emulator, &name).await;
        let pending = entries(&in_flight, "PendingProperties", case);
        assert!(
            entries(&in_flight, "CompletedProperties", case).is_empty(),
            "case {case:?}: nothing has completed while the change is in flight, got \
             {in_flight:?}"
        );

        // The clock, not the reader, lands the change: wait for the condition.
        settled_domain(&emulator, &name).await;
        let settled = progress(&emulator, &name).await;
        assert_eq!(
            entries(&settled, "CompletedProperties", case),
            pending,
            "case {case:?}: the properties reported as pending are the ones reported as \
             completed, got {settled:?}"
        );
        assert!(
            entries(&settled, "PendingProperties", case).is_empty(),
            "case {case:?}: nothing is pending once the change has landed, got {settled:?}"
        );
    }
}
