// SPDX-License-Identifier: CC0-1.0

//! The service-configuration projection every operation that reports one answers
//! with, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rules under test, each asserted over a TABLE of configuration documents whose
//! rows differ along the number of items, which item declares parameters, and how many
//! parameters and values each one carries:
//!
//! * a reported configuration document carries every item it holds with the parameters
//!   attached to that item, and carries only the members the model declares for it, so
//!   nothing restates a parameter beside the item it belongs to;
//! * an operation answers exactly the members its modelled output declares: a read of
//!   one group's configuration answers the document alone, and an accepted put answers
//!   the empty document;
//! * applying a configuration writes it, so the projection a later read reports is the
//!   one the put accepted rather than an empty document.

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroups_ardi::state::{
    ALLOWED_RESOURCE_TYPES_PARAMETER, CONFIGURATION_IN_EFFECT, CONFIGURATION_UPDATING,
    DELETION_PROTECTION_PARAMETER, DELETION_PROTECTION_UNLESS_EMPTY,
};

use super::put_group_configuration::make_update_due;
use super::resource_group_lifecycle::Emulator;

/// One row: a `Configuration` document, exactly as a request carries it.
struct ConfigurationCase {
    label: &'static str,
    configuration: Value,
}

/// The members the modelled `GroupConfiguration` structure declares
/// (`smithy_query(query="shape",shape="GroupConfiguration",model_path="services/service-com-amazonaws-resourcegroups-ardi/smithy-model/com-amazonaws-resourcegroups-ardi.json")`).
const CONFIGURATION_MEMBERS: [&str; 4] = [
    "Configuration",
    "ProposedConfiguration",
    "Status",
    "FailureReason",
];

/// The members the modelled `GetGroupConfigurationOutput` declares
/// (`smithy_query(query="shape",shape="GetGroupConfigurationOutput",model_path="services/service-com-amazonaws-resourcegroups-ardi/smithy-model/com-amazonaws-resourcegroups-ardi.json")`).
const GET_OUTPUT_MEMBERS: [&str; 1] = ["GroupConfiguration"];

/// Every configuration document the rules are asserted over.
///
/// The configuration item types and the parameter names are AWS service-configuration
/// vocabulary from the "Service configurations for resource groups" reference, which is
/// what `GroupConfigurationItem$Type` and `GroupConfigurationParameter$Name` are
/// documented against; two of the names are the ones this service already decides rules
/// by (`ALLOWED_RESOURCE_TYPES_PARAMETER`, `DELETION_PROTECTION_PARAMETER`), and
/// `allowed-host-families` is a third from the same reference. A document holds at most
/// two items (`smithy_query(query="shape",shape="GroupConfigurationList")`).
fn configuration_cases() -> Vec<ConfigurationCase> {
    let allowed = ALLOWED_RESOURCE_TYPES_PARAMETER;
    let protection = DELETION_PROTECTION_PARAMETER;
    let unless_empty = DELETION_PROTECTION_UNLESS_EMPTY;
    vec![
        ConfigurationCase {
            label: "one item, no parameters at all",
            configuration: json!([{ "Type": "AWS::EC2::CapacityReservationPool" }]),
        },
        ConfigurationCase {
            label: "one item, one parameter with one value",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [{ "Name": protection, "Values": [unless_empty] }]
            }]),
        },
        ConfigurationCase {
            label: "two items, the parameters on the second",
            configuration: json!([
                { "Type": "AWS::EC2::CapacityReservationPool" },
                {
                    "Type": "AWS::ResourceGroups::Generic",
                    "Parameters": [
                        { "Name": allowed, "Values": ["AWS::EC2::CapacityReservation"] }
                    ]
                }
            ]),
        },
        ConfigurationCase {
            label: "two items, the parameters on the first, two values",
            configuration: json!([
                {
                    "Type": "AWS::ResourceGroups::Generic",
                    "Parameters": [{
                        "Name": allowed,
                        "Values": ["AWS::EC2::CapacityReservation", "AWS::EC2::Instance"]
                    }]
                },
                { "Type": "AWS::EC2::CapacityReservationPool" }
            ]),
        },
        ConfigurationCase {
            label: "one item carrying two parameters",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [
                    { "Name": allowed, "Values": ["AWS::EC2::CapacityReservation"] },
                    { "Name": protection, "Values": [unless_empty] }
                ]
            }]),
        },
        ConfigurationCase {
            label: "both items carrying parameters",
            configuration: json!([
                {
                    "Type": "AWS::EC2::HostManagement",
                    "Parameters": [{ "Name": "allowed-host-families", "Values": ["c5", "m5"] }]
                },
                {
                    "Type": "AWS::ResourceGroups::Generic",
                    "Parameters": [{ "Name": protection, "Values": [unless_empty] }]
                }
            ]),
        },
        ConfigurationCase {
            label: "a parameter that names no value",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [{ "Name": allowed }]
            }]),
        },
        ConfigurationCase {
            label: "a parameter whose value list is empty",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [{ "Name": allowed, "Values": [] }]
            }]),
        },
    ]
}

/// Assert that the JSON object `document` carries only members in `modelled`, which is
/// the member list the model declares for the shape it is an instance of.
fn assert_only_modelled_members(case: &str, source: &str, modelled: &[&str], document: &Value) {
    let members = document.as_object().unwrap_or_else(|| {
        panic!("case {case:?}: expected {source} to answer a structure, got {document}")
    });
    let unmodelled: Vec<&str> = members
        .keys()
        .map(String::as_str)
        .filter(|member| !modelled.contains(member))
        .collect();
    assert!(
        unmodelled.is_empty(),
        "case {case:?}: {source} reported members the model does not declare \
         ({unmodelled:?}), got {document}",
    );
}

/// The group a response created, and the ARN it was stored under.
async fn create(emulator: &Emulator, request: &Value) -> (Value, String) {
    let created = emulator.ok("/groups", request).await;
    let arn = created["Group"]["GroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a created group reports its ARN, got {created}"))
        .to_string();
    (created, arn)
}

/// A create echoes the whole configuration document — every item with its own
/// parameters — and a read of that group answers the same document as the only member
/// its modelled output declares, whether the request addressed the group by name or by
/// ARN.
#[tokio::test]
async fn a_reported_configuration_carries_its_items_and_only_its_modelled_members() {
    for case in configuration_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        let name = "configuration-projection";
        let (created, arn) = create(
            &emulator,
            &json!({ "Name": name, "Configuration": case.configuration }),
        )
        .await;
        let echoed = created["GroupConfiguration"].clone();
        assert_eq!(
            echoed["Configuration"], case.configuration,
            "case {label:?}: expected the create to echo every configuration item with its \
             own parameters, got {echoed}",
        );
        assert_only_modelled_members(
            label,
            "the GroupConfiguration a create reports",
            &CONFIGURATION_MEMBERS,
            &echoed,
        );
        assert_eq!(
            echoed["Status"],
            json!(CONFIGURATION_IN_EFFECT),
            "case {label:?}: the configuration a create stores is the one in effect, got \
             {echoed}",
        );

        // The same document on every read of it — whichever form the request
        // addressed the group in — and nothing beside it.
        for (addressed, identifying) in [("by name", name), ("by ARN", arn.as_str())] {
            let read = emulator
                .ok("/get-group-configuration", &json!({ "Group": identifying }))
                .await;
            assert_only_modelled_members(
                label,
                &format!("GetGroupConfiguration {addressed}"),
                &GET_OUTPUT_MEMBERS,
                &read,
            );
            assert_eq!(
                read["GroupConfiguration"], echoed,
                "case {label:?} ({addressed}): expected the read to report the document the \
                 create reported, got {read}",
            );
        }
    }
}

/// A put is accepted with the empty document its model declares, and the projection it
/// wrote is what a read then reports: the accepted document as the proposed one while
/// the update is in flight, and the one in effect once the clock owes the update.
#[tokio::test]
async fn applying_a_configuration_answers_the_empty_document_and_writes_the_projection() {
    for case in configuration_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        let name = "configuration-applied";
        // A group created with neither a configuration nor a query: it holds none in
        // effect until this row's put attaches one.
        create(&emulator, &json!({ "Name": name })).await;

        let (status, answered) = emulator
            .call(
                "/put-group-configuration",
                &json!({ "Group": name, "Configuration": case.configuration }),
            )
            .await;
        assert_eq!(
            status, 202,
            "case {label:?}: expected an accepted put to answer 202, got {status} {answered}",
        );
        assert!(
            is_empty_document(&answered),
            "case {label:?}: PutGroupConfiguration declares no output member, so an \
             accepted put answers the empty document, got {answered}",
        );

        // While the update is in flight it is the proposed configuration, and the
        // read answers the document alone.
        let in_flight = emulator
            .ok("/get-group-configuration", &json!({ "Group": name }))
            .await;
        assert_only_modelled_members(
            label,
            "GetGroupConfiguration while the update is in flight",
            &GET_OUTPUT_MEMBERS,
            &in_flight,
        );
        let applied = in_flight["GroupConfiguration"].clone();
        assert_only_modelled_members(
            label,
            "the GroupConfiguration reported while the update is in flight",
            &CONFIGURATION_MEMBERS,
            &applied,
        );
        assert_eq!(
            applied["ProposedConfiguration"], case.configuration,
            "case {label:?}: expected the configuration the put accepted to be the proposed \
             one, got {applied}",
        );
        assert_eq!(
            applied["Status"],
            json!(CONFIGURATION_UPDATING),
            "case {label:?}: expected the update to be reported as in flight, got {applied}",
        );

        // Once the clock owes the update, the document it carried is the one in
        // effect — items and parameters as the put carried them.
        make_update_due(&emulator, name);
        let read = emulator
            .ok("/get-group-configuration", &json!({ "Group": name }))
            .await;
        assert_eq!(
            read["GroupConfiguration"]["Configuration"], case.configuration,
            "case {label:?}: expected the applied configuration in effect, got {read}",
        );
        assert_only_modelled_members(
            label,
            "the GroupConfiguration reported once the update is in effect",
            &CONFIGURATION_MEMBERS,
            &read["GroupConfiguration"],
        );
    }
}

/// Whether a response carries no member at all: an empty body, or the empty JSON
/// document the restJson1 wire writes for an output shape that declares none.
pub(crate) fn is_empty_document(body: &Value) -> bool {
    body.is_null() || body.as_object().is_some_and(|members| members.is_empty())
}
