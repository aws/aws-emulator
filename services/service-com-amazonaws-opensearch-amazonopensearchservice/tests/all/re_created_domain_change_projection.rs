// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a `CreateDomain` answer says about the configuration change
//! the create started when the create is of a domain name the service has HELD BEFORE.
//!
//! Every case drives the real wire path — a `restJson1` request through the multi-service
//! router into the mounted service — so what is asserted is what a client sees. The
//! harness, the smallest create body, the settle poll and the submitted access policy come
//! from [`crate::domain_lifecycle`], [`crate::successive_domain_transitions`] and
//! [`crate::change_property_enumeration`]; this suite adds no second copy of them.
//!
//! ONE rule:
//!
//! > what a descriptor says about the configuration change a create started — the status
//! > that change is in and the properties it is modifying — is read off the stored domain
//! > and off that change's own progress, so a create under a name the service has held
//! > before is projected exactly as a first create is: the same change status for the change
//! > it started, the same enumerated properties in the same order, the same value and kind
//! > for each of them, the submitted access policy echoed as it was written, and the same
//! > span of work owed before the change lands.
//!
//! The axis the rows span is the HISTORY the name carries into the measured create: a name
//! the service never held, a second such name, a name re-created the instant its domain was
//! deleted, a name re-created after its domain had settled first, and a name on its third
//! domain. WHICH properties a change enumerates and how each value is encoded is
//! [`crate::change_property_enumeration`]'s table, over a set of create bodies; what is new
//! here is that none of it — and no part of the change's own status — moves with a name's
//! history, which is what a client that creates a domain, tears it down and creates it
//! again depends on.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::CONFIG_CHANGE_STATUS_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::CONFIG_CHANGE_COMPLETED;

use crate::change_property_enumeration::submitted_policy;
use crate::domain_lifecycle::{settled_domain, string_member, Emulator};
use crate::successive_domain_transitions::single_node_create;

/// `PropertyValueType`'s two values, as `ModifyingProperties$ValueType` reports them
/// (`smithy_query(query="shape",shape="com.amazonaws.opensearch#PropertyValueType")`).
const STRINGIFIED_JSON: &str = "STRINGIFIED_JSON";
const PLAIN_TEXT: &str = "PLAIN_TEXT";

/// One entry of a `ModifyingProperties` list, as a client reads it:
/// `ModifyingProperties{Name, ActiveValue, PendingValue, ValueType}`.
#[derive(Debug, PartialEq, Eq)]
struct Property {
    name: String,
    active_value: String,
    pending_value: String,
    value_type: String,
}

/// What a descriptor of a domain says about the configuration change in flight on it: which
/// change it is, the status that change is in, and the properties it is modifying.
#[derive(Debug)]
struct Projection {
    change_id: String,
    config_change_status: String,
    properties: Vec<Property>,
}

/// The [`Projection`] a `DomainStatus` carries.
fn projection(status: &Value, case: &str) -> Projection {
    let details = &status["ChangeProgressDetails"];
    let properties = status["ModifyingProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected ModifyingProperties, got {status:?}"))
        .iter()
        .map(|entry| Property {
            name: string_member(entry, "Name", case),
            active_value: string_member(entry, "ActiveValue", case),
            pending_value: string_member(entry, "PendingValue", case),
            value_type: string_member(entry, "ValueType", case),
        })
        .collect();
    Projection {
        change_id: string_member(details, "ChangeId", case),
        config_change_status: string_member(details, "ConfigChangeStatus", case),
        properties,
    }
}

/// The entry a projection carries for the property `name`, or `None` when it carries none.
fn entry<'a>(reported: &'a Projection, name: &str) -> Option<&'a Property> {
    reported
        .properties
        .iter()
        .find(|property| property.name == name)
}

/// The `CreateDomain` body every generation of a name is created with: the smallest domain
/// this control plane provisions, carrying a submitted access policy, fine-grained access
/// control switched off, and the tags of `tagged` rows.
fn create_body(name: &str, tagged: bool) -> Value {
    let mut body = single_node_create(name);
    body["AccessPolicies"] = Value::from(submitted_policy());
    body["AdvancedSecurityOptions"] = json!({ "Enabled": false });
    if tagged {
        body["TagList"] = json!([
            { "Key": "Owner", "Value": "teamA" },
            { "Key": "Env", "Value": "staging" },
        ]);
    }
    body
}

/// The configurations the rule is held over: the one the recorded re-create carries — a
/// policy, no tags and fine-grained access control off — and the same create carrying tags,
/// so the comparison is not made on a single body.
fn configurations() -> Vec<(&'static str, bool)> {
    vec![
        ("a create carrying a policy and no tags", false),
        ("a create carrying a policy and two tags", true),
    ]
}

/// One name's history before the create whose answer is measured: how many domains the name
/// held before it, and whether each of those was left to settle before it was deleted.
struct Lineage {
    case: &'static str,
    earlier_generations: usize,
    settled_first: bool,
}

/// The rows: a name the service never held, one re-created the instant its domain was
/// deleted, one re-created after its domain had settled, and one on its third domain.
fn lineages() -> Vec<Lineage> {
    vec![
        Lineage {
            case: "a second name the service has never held",
            earlier_generations: 0,
            settled_first: false,
        },
        Lineage {
            case: "a name re-created the instant its domain was deleted",
            earlier_generations: 1,
            settled_first: false,
        },
        Lineage {
            case: "a name re-created after its domain had settled",
            earlier_generations: 1,
            settled_first: true,
        },
        Lineage {
            case: "a name on its third domain",
            earlier_generations: 2,
            settled_first: false,
        },
    ]
}

/// `CreateDomain` under `name`, asserting it was accepted, and the `DomainStatus` it
/// answered with.
async fn created(emulator: &Emulator, name: &str, tagged: bool, case: &str) -> Value {
    let answer = emulator.create(create_body(name, tagged)).await;
    assert_eq!(
        answer.status, 200,
        "case {case} (CreateDomain({name:?})): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"].clone()
}

/// `DeleteDomain` of `name`, asserting it was accepted.
async fn deleted(emulator: &Emulator, name: &str, case: &str) {
    let answer = emulator.delete(name).await;
    assert_eq!(
        answer.status, 200,
        "case {case} (DeleteDomain({name:?})): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Walk `name` through the generations `lineage` asks for and answer the change ids those
/// earlier creates named, so the measured create can be held to naming a change of its own.
async fn earlier_generations(
    emulator: &Emulator,
    name: &str,
    tagged: bool,
    lineage: &Lineage,
) -> Vec<String> {
    let mut named = Vec::new();
    for _ in 0..lineage.earlier_generations {
        let status = created(emulator, name, tagged, lineage.case).await;
        named.push(string_member(
            &status["ChangeProgressDetails"],
            "ChangeId",
            lineage.case,
        ));
        if lineage.settled_first {
            // The condition a client waits on, polled for rather than waited out, so the
            // case does not depend on how fast the host runs.
            settled_domain(emulator, name).await;
        }
        deleted(emulator, name, lineage.case).await;
    }
    named
}

/// RULE: a create under a name the service has held before projects the change it started
/// exactly as a first create does.
#[tokio::test]
async fn a_re_created_domain_projects_its_change_as_a_first_create_does() {
    let emulator = Emulator::new();

    for (index, (configuration, tagged)) in configurations().into_iter().enumerate() {
        // The baseline every row is measured against: the FIRST create of a name the
        // service has never held.
        let baseline_name = format!("first-create-{index}");
        let baseline_case = format!("{configuration}, a name the service has never held");
        let baseline = projection(
            &created(&emulator, &baseline_name, tagged, &baseline_case).await,
            &baseline_case,
        );
        assert!(
            CONFIG_CHANGE_STATUS_VALUES.contains(&baseline.config_change_status.as_str()),
            "case {baseline_case}: expected one of the modelled change statuses \
             {CONFIG_CHANGE_STATUS_VALUES:?}, got {:?}",
            baseline.config_change_status
        );

        for (row, lineage) in lineages().iter().enumerate() {
            let case = format!("{configuration}, {}", lineage.case);
            let name = format!("re-created-{index}-{row}");
            let earlier = earlier_generations(&emulator, &name, tagged, lineage).await;

            // The measured create: the one whose answer the rule is about.
            let measured = projection(&created(&emulator, &name, tagged, &case).await, &case);

            // It started a change of its OWN — no earlier generation's change is reported
            // as the one in flight.
            assert!(
                !earlier.contains(&measured.change_id),
                "case {case}: expected a change of its own, got {:?} among the earlier \
                 generations' changes {earlier:?}",
                measured.change_id
            );
            // ... reported in the status a create's change is in, whatever the name held
            // before.
            assert_eq!(
                measured.config_change_status, baseline.config_change_status,
                "case {case}: expected the change status a first create reports \
                 ({:?}), got {:?}",
                baseline.config_change_status, measured.config_change_status
            );
            // ... and modifying the same properties, in the same order, each with the same
            // value and the same kind: a client indexing the list, or looking a property up
            // by name, reads what it read on the first create.
            assert_eq!(
                measured.properties, baseline.properties,
                "case {case}: expected the properties a first create reports as being \
                 modified, got {:?}",
                measured.properties
            );

            // And the entries the recorded re-create discloses, asserted on their own rather
            // than only against the baseline: a feature fine-grained access control leaves
            // off is the toggle `false` as plain text, an option the service applies whole
            // carries its own document, and a cluster property the configuration does not use
            // has no value at all.
            let mut expected = vec![
                ("AdvancedSecurityOptions.AnonymousAuthDisableDate", "false", PLAIN_TEXT),
                ("AdvancedSecurityOptions.AnonymousAuthEnabled", "false", PLAIN_TEXT),
                ("AdvancedSecurityOptions.IAMFederationOptions", "false", PLAIN_TEXT),
                ("AdvancedSecurityOptions.InternalUserDatabaseEnabled", "false", PLAIN_TEXT),
                ("AdvancedSecurityOptions.JWTOptions", "false", PLAIN_TEXT),
                ("AdvancedSecurityOptions.MasterUserOptions", "false", PLAIN_TEXT),
                ("AdvancedSecurityOptions.SAMLOptions", "false", PLAIN_TEXT),
                ("ClusterConfig.ColdStorageOptions", "{\"Enabled\":false}", STRINGIFIED_JSON),
                ("ClusterConfig.DedicatedMasterCount", "", PLAIN_TEXT),
                ("AutomatedSnapshotPauseOptions", "{\"Enabled\":false}", STRINGIFIED_JSON),
            ];
            if !tagged {
                // A create that carried no tag leaves the tag set with no value, which is no
                // JSON document for a caller to parse.
                expected.push(("TAGS", "", PLAIN_TEXT));
            }
            for (property, pending_value, value_type) in expected {
                assert_eq!(
                    entry(&measured, property),
                    Some(&Property {
                        name: property.to_owned(),
                        active_value: String::new(),
                        pending_value: pending_value.to_owned(),
                        value_type: value_type.to_owned(),
                    }),
                    "case {case} ({property}): expected the value the property is moving to \
                     ({pending_value:?} as {value_type}), got {:?}",
                    entry(&measured, property)
                );
            }

            // The document the caller submitted is the value the policy is moving to, with
            // the principal in the form the request wrote it in — a re-create echoes the
            // caller's policy, not a re-authored one.
            let policy = entry(&measured, "AccessPolicies")
                .unwrap_or_else(|| panic!("case {case}: expected an AccessPolicies entry"));
            assert_eq!(
                (
                    ::serde_json::from_str::<Value>(&policy.pending_value).ok(),
                    policy.value_type.as_str(),
                ),
                (
                    ::serde_json::from_str::<Value>(&submitted_policy()).ok(),
                    STRINGIFIED_JSON,
                ),
                "case {case} (AccessPolicies): expected the submitted document, got {policy:?}"
            );
            assert!(
                policy
                    .pending_value
                    .contains(&format!("\"AWS\":\"{}\"", ::engine::emulator_account_id())),
                "case {case} (AccessPolicies): expected the principal as submitted, got {:?}",
                policy.pending_value
            );
        }
    }
}

/// RULE (the same one, on the span): the change a re-create starts is owed the same work a
/// first create's is — it lands as the SAME change, reported completed, with nothing left
/// being modified.
#[tokio::test]
async fn a_re_created_domain_s_change_lands_as_the_change_the_create_named() {
    let emulator = Emulator::new();

    // Two rows: a name the service has never held, and the same name once its first domain
    // has been deleted — the lineage the recorded re-create carries.
    let name = "lands-after-re-create";
    let first = projection(
        &created(&emulator, name, false, "a first create").await,
        "a first create",
    );
    deleted(&emulator, name, "a first create").await;
    let again = projection(
        &created(&emulator, name, false, "a re-create").await,
        "a re-create",
    );
    assert_ne!(
        again.change_id, first.change_id,
        "case a re-create: expected a change of its own, got the first create's {:?}",
        first.change_id
    );

    let settled = settled_domain(&emulator, name).await;
    let landed = projection(&settled, "a re-create, settled");
    assert_eq!(
        (landed.change_id.as_str(), landed.config_change_status.as_str()),
        (again.change_id.as_str(), CONFIG_CHANGE_COMPLETED),
        "case a re-create, settled: expected the change the create named, reported \
         {CONFIG_CHANGE_COMPLETED}, got {landed:?}"
    );
    assert!(
        landed.properties.is_empty(),
        "case a re-create, settled: a domain with no change in flight modifies nothing, got \
         {:?}",
        landed.properties
    );
}
