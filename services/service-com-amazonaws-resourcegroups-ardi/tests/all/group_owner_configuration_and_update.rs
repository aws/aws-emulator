// SPDX-License-Identifier: CC0-1.0

//! The `Group` projection, service-configuration groups, and `UpdateGroup`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * every `Group` structure a response carries reports exactly the members the model
//!   declares for it — both required ones always, and nothing the model does not declare
//!   — whatever the operation that answers and whatever the group was created with;
//! * a group created with a service `Configuration` is an ordinary group: it is
//!   stored, reports the configuration it holds, and resolves by name or by ARN
//!   through every operation that addresses one group, up to and including its
//!   `DeleteGroup`;
//! * `UpdateGroup` persists every attribute the request carries and leaves the
//!   stored value of every attribute it omits alone, so a later read returns the
//!   updated group.

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, identifier_names, Emulator, ACCOUNT};

/// The attributes `UpdateGroup` can change, which are also the `Group` members
/// every assertion below compares (`UpdateGroupInput` carries exactly these four
/// beside the group identifier).
const UPDATABLE: [&str; 4] = ["Description", "Criticality", "Owner", "DisplayName"];

/// The members the modelled `Group` structure declares
/// (`smithy_query(query="shape",shape="Group",model_path="services/service-com-amazonaws-resourcegroups-ardi/smithy-model/com-amazonaws-resourcegroups-ardi.json")`),
/// the two `@required` ones first.
const GROUP_MEMBERS: [&str; 7] = [
    "GroupArn",
    "Name",
    "Description",
    "Criticality",
    "Owner",
    "DisplayName",
    "ApplicationTag",
];

/// The account segment of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`).
fn arn_account(arn: &str) -> &str {
    arn.split(':').nth(4).unwrap_or("<no account segment>")
}

/// Assert that `group` — a `Group` structure answered by `source` — carries only
/// members the model declares, reports the two required ones, and names the account
/// that owns it in the ARN that identifies it.
fn assert_modelled_group(case: &str, source: &str, group: &Value) {
    let members = group.as_object().unwrap_or_else(|| {
        panic!("case {case:?}: {source} must answer a Group structure, got {group}")
    });
    let unmodelled: Vec<&str> = members
        .keys()
        .map(String::as_str)
        .filter(|member| !GROUP_MEMBERS.contains(member))
        .collect();
    assert!(
        unmodelled.is_empty(),
        "case {case:?}: {source} reported Group members the model does not declare \
         ({unmodelled:?}), got {group}",
    );
    for required in ["GroupArn", "Name"] {
        assert!(
            members
                .get(required)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty()),
            "case {case:?}: {source} must report Group.{required}, got {group}",
        );
    }
    let arn = group["GroupArn"].as_str().unwrap_or("<no GroupArn>");
    assert_eq!(
        arn_account(arn),
        ACCOUNT,
        "case {case:?}: {source} must identify the group by an ARN naming the owning \
         account {ACCOUNT}, got {arn}",
    );
}

/// The `Group` member `member` of `body`, as the response carries it.
fn group_member(body: &Value, member: &str) -> Value {
    body["Group"].get(member).cloned().unwrap_or(Value::Null)
}

/// Every operation that answers a `Group` answers the modelled projection of it: the
/// create, the read, the update, the (deprecated) listing and the delete.
#[tokio::test]
async fn every_group_payload_reports_only_the_modelled_members() {
    // (label, the CreateGroup request, the region its credential scope names)
    let cases = vec![
        (
            "only the required Name",
            json!({ "Name": "owner-minimal" }),
            "us-east-1",
        ),
        (
            "every optional attribute set",
            json!({
                "Name": "owner-full",
                "Description": "Probe tag query group",
                "Criticality": 3,
                "Owner": "owner@example.com",
                "DisplayName": "Owner Full",
            }),
            "us-east-1",
        ),
        (
            "a tag-filter resource query",
            json!({
                "Name": "owner-query",
                "ResourceQuery": {
                    "Type": "TAG_FILTERS_1_0",
                    "Query": "{\"ResourceTypeFilters\":[\"AWS::AllSupported\"],\"TagFilters\":[]}"
                }
            }),
            "us-east-1",
        ),
        (
            "a service configuration",
            json!({
                "Name": "owner-configured",
                "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }]
            }),
            "us-east-1",
        ),
        (
            // The account is the one every ARN this emulator mints names, so a group
            // minted in another region is identified the same way.
            "another region in the credential scope",
            json!({ "Name": "owner-elsewhere" }),
            "eu-west-1",
        ),
    ];

    for (case, request, region) in cases {
        let emulator = Emulator::new();
        let name = request["Name"].as_str().expect("the case names the group");
        let (status, created) = emulator.call_in(region, "/groups", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateGroup to answer 200, got {status} {created}",
        );
        assert_modelled_group(case, "CreateGroup", &created["Group"]);

        let read = emulator
            .ok_in(region, "/get-group", &json!({ "Group": name }))
            .await;
        assert_modelled_group(case, "GetGroup", &read["Group"]);

        let listed = emulator.ok_in(region, "/groups-list", &json!({})).await;
        assert_eq!(
            identifier_names(&listed),
            vec![name.to_string()],
            "case {case:?}: expected ListGroups to report exactly [{name}], got {listed}",
        );
        assert_modelled_group(
            case,
            "the deprecated ListGroups$Groups",
            &listed["Groups"][0],
        );

        let updated = emulator
            .ok_in(
                region,
                "/update-group",
                &json!({ "Group": name, "Description": "Updated probe group" }),
            )
            .await;
        assert_modelled_group(case, "UpdateGroup", &updated["Group"]);

        let deleted = emulator
            .ok_in(region, "/delete-group", &json!({ "Group": name }))
            .await;
        assert_modelled_group(case, "DeleteGroup", &deleted["Group"]);
    }
}

/// One row of the configuration table: a whole `Configuration` list.
struct ConfigurationCase {
    label: &'static str,
    name: &'static str,
    configuration: Value,
}

/// The configurations a group can be created with.
///
/// The `Type` values and the parameter names are AWS service-configuration types
/// from the "Service configurations for Resource Groups" reference
/// (<https://docs.aws.amazon.com/ARG/latest/APIReference/about-slg.html>), which is
/// the vocabulary `GroupConfigurationItem$Type` is documented in and whose shape
/// its `^AWS::[a-zA-Z0-9]+::[a-zA-Z0-9]+$` pattern requires; the list's 0..2 length
/// and the parameter `Name` pattern `^[a-z-]+$` come from the model shapes
/// `GroupConfigurationList` and `GroupConfigurationParameterName`.
fn configuration_cases() -> Vec<ConfigurationCase> {
    vec![
        ConfigurationCase {
            label: "one item, no parameters",
            name: "config-bare",
            configuration: json!([{ "Type": "AWS::ResourceGroups::Generic" }]),
        },
        ConfigurationCase {
            label: "one item with a parameter carrying one value",
            name: "config-parameterized",
            configuration: json!([{
                "Type": "AWS::EC2::HostManagement",
                "Parameters": [
                    { "Name": "any-host-based-license-configuration", "Values": ["true"] }
                ]
            }]),
        },
        ConfigurationCase {
            label: "two items, the modelled maximum, with several parameter values",
            name: "config-pair",
            configuration: json!([
                {
                    "Type": "AWS::EC2::HostManagement",
                    "Parameters": [{ "Name": "auto-allocate-host", "Values": ["true"] }]
                },
                {
                    "Type": "AWS::ResourceGroups::Generic",
                    "Parameters": [
                        { "Name": "allowed-resource-types", "Values": ["AWS::EC2::Host"] },
                        { "Name": "deletion-protection", "Values": ["UNLESS_EMPTY"] }
                    ]
                }
            ]),
        },
    ]
}

/// A group created with a service `Configuration` is stored like any other group:
/// it reports the configuration it holds, reads back through `GetGroup` and
/// `GetGroupConfiguration`, appears in the listing, and its `DeleteGroup` — by ARN
/// — answers 200 with the deleted `Group`.
#[tokio::test]
async fn a_group_created_with_a_service_configuration_is_an_ordinary_group() {
    for case in configuration_cases() {
        let label = case.label;
        let name = case.name;
        let emulator = Emulator::new();

        let created = emulator
            .ok(
                "/groups",
                &json!({
                    "Name": name,
                    "Description": "Probe configuration group",
                    "Configuration": case.configuration,
                }),
            )
            .await;
        let arn = created["Group"]["GroupArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: CreateGroup returned no GroupArn: {created}"))
            .to_string();
        assert_eq!(
            created["GroupConfiguration"]["Configuration"], case.configuration,
            "case {label:?}: expected the created group to report the configuration it was \
             created with ({}), got {}",
            case.configuration, created["GroupConfiguration"],
        );
        // The configuration a create stores is the one in effect: nothing is being
        // applied, so there is no proposed configuration and no failure reason.
        assert_eq!(
            created["GroupConfiguration"]["Status"].as_str(),
            Some("UPDATE_COMPLETE"),
            "case {label:?}: expected Status UPDATE_COMPLETE, got {}",
            created["GroupConfiguration"],
        );
        assert!(
            created["GroupConfiguration"]
                .get("ProposedConfiguration")
                .is_none(),
            "case {label:?}: a configuration in effect has none proposed, got {}",
            created["GroupConfiguration"],
        );
        // A group contains either a configuration or a resource query.
        assert!(
            created.get("ResourceQuery").is_none(),
            "case {label:?}: a configured group carries no ResourceQuery, got {created}",
        );

        // The stored configuration reads back, by name and by ARN.
        for identifier in [name.to_string(), arn.clone()] {
            let fetched = emulator
                .ok("/get-group-configuration", &json!({ "Group": identifier }))
                .await;
            assert_eq!(
                fetched["GroupConfiguration"], created["GroupConfiguration"],
                "case {label:?}: GetGroupConfiguration for {identifier} returned {}, \
                 expected {}",
                fetched["GroupConfiguration"], created["GroupConfiguration"],
            );
        }

        // And so does the group itself, through every reader.
        let read = emulator.ok("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            read["Group"], created["Group"],
            "case {label:?}: GetGroup returned {}, expected {}",
            read["Group"], created["Group"],
        );
        let listed = emulator.ok("/groups-list", &json!({})).await;
        assert_eq!(
            identifier_names(&listed),
            vec![name.to_string()],
            "case {label:?}: expected ListGroups to report exactly [{name}], got {listed}",
        );

        // The teardown the recording makes: a delete by the group's ARN answers 200
        // with the deleted group, not 404.
        let deleted = emulator.ok("/delete-group", &json!({ "Group": arn })).await;
        assert_eq!(
            deleted["Group"], created["Group"],
            "case {label:?}: expected DeleteGroup by ARN to return {}, got {}",
            created["Group"], deleted["Group"],
        );
        let (status, body) = emulator.call("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "NotFoundException"),
            "case {label:?}: expected a deleted configuration group to be 404 \
             NotFoundException, got {status} {body}",
        );
    }
}

/// A `Configuration` that configures nothing, and the configurations a create
/// refuses. A present but EMPTY list carries no configuration item, so the group
/// is stored exactly as one that sent none — and a group that holds no service
/// configuration has none to get.
#[tokio::test]
async fn an_empty_configuration_configures_nothing_and_a_conflicting_one_is_refused() {
    let emulator = Emulator::new();
    let created = emulator
        .ok("/groups", &json!({ "Name": "config-empty", "Configuration": [] }))
        .await;
    assert!(
        created.get("GroupConfiguration").is_none(),
        "an empty Configuration configures nothing, so the create reports none: {created}",
    );
    let (status, body) = emulator
        .call(
            "/get-group-configuration",
            &json!({ "Group": "config-empty" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "a group that holds no service configuration has none to get: got {status} {body}",
    );
    // The group itself is there: only its configuration is not.
    let read = emulator
        .ok("/get-group", &json!({ "Group": "config-empty" }))
        .await;
    assert_eq!(
        read["Group"]["Name"].as_str(),
        Some("config-empty"),
        "the group a create with an empty Configuration stored must still read back, \
         got {read}",
    );

    // (label, the CreateGroup request)
    let refused = vec![
        (
            "a Configuration and a ResourceQuery in one request",
            json!({
                "Name": "config-and-query",
                "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }],
                "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "{}" }
            }),
        ),
        (
            "two configuration items with the same Type",
            json!({
                "Name": "config-duplicate",
                "Configuration": [
                    { "Type": "AWS::ResourceGroups::Generic" },
                    { "Type": "AWS::ResourceGroups::Generic" }
                ]
            }),
        ),
        (
            // One past the modelled `GroupConfigurationList` 0..2 length.
            "three configuration items",
            json!({
                "Name": "config-too-many",
                "Configuration": [
                    { "Type": "AWS::ResourceGroups::Generic" },
                    { "Type": "AWS::EC2::HostManagement" },
                    { "Type": "AWS::EC2::CapacityReservationPool" }
                ]
            }),
        ),
        (
            "a configuration item Type outside the modelled pattern",
            json!({
                "Name": "config-bad-type",
                "Configuration": [{ "Type": "not a configuration type" }]
            }),
        ),
    ];
    for (label, request) in refused {
        let (status, body) = emulator.call("/groups", &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {label:?}: expected 400 BadRequestException, got {status} {body}",
        );
        let name = request["Name"].as_str().expect("the case names the group");
        let (status, read) = emulator.call("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a refused create stores nothing, so {name} must not exist, \
             got {status} {read}",
        );
    }

    // A configuration is addressed through its group, so an identifier that names
    // no stored group is `NotFoundException`, and a request naming none at all is a
    // `BadRequestException`.
    for (label, request, expected) in [
        (
            "a group that does not exist",
            json!({ "Group": "no-such-group" }),
            (404, "NotFoundException"),
        ),
        (
            "an ARN for another account",
            json!({ "Group": "arn:aws:resource-groups:us-east-1:111122223333:group/config-empty" }),
            (404, "NotFoundException"),
        ),
        (
            "no group at all",
            json!({}),
            (400, "BadRequestException"),
        ),
    ] {
        let (status, body) = emulator.call("/get-group-configuration", &request).await;
        assert_eq!(
            (status, error_code(&body)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} {body}",
        );
    }
}

/// One row of the update table: the members `UpdateGroup` carries beside the
/// group identifier.
struct UpdateCase {
    label: &'static str,
    update: Value,
}

/// The create every update case starts from, so each row's expected result is the
/// base overlaid with that row's update.
fn update_base() -> Value {
    json!({
        "Name": "updatable-group",
        "Description": "Probe tag query group",
        "Criticality": 5,
        "Owner": "base-owner@example.com",
        "DisplayName": "Base Display",
    })
}

/// `UpdateGroup` persists every attribute the request carries and leaves every
/// attribute it omits alone, and the stored group is what every later read
/// returns.
#[tokio::test]
async fn update_group_persists_each_attribute_and_leaves_the_others_alone() {
    let base = update_base();
    let name = base["Name"].as_str().expect("the base names the group");
    // The modelled `Description` length is 0..1024, so both bounds are a legal
    // update; `Criticality` is a 1..10 range.
    let longest_description = "d".repeat(1024);
    let cases = vec![
        UpdateCase {
            label: "a new description",
            update: json!({ "Description": "Updated probe group" }),
        },
        UpdateCase {
            label: "a description cleared to the empty string",
            update: json!({ "Description": "" }),
        },
        UpdateCase {
            label: "a description at the modelled 1024 maximum",
            update: json!({ "Description": longest_description }),
        },
        UpdateCase {
            label: "criticality at the most critical bound",
            update: json!({ "Criticality": 1 }),
        },
        UpdateCase {
            label: "criticality at the least critical bound",
            update: json!({ "Criticality": 10 }),
        },
        UpdateCase {
            label: "a new owner",
            update: json!({ "Owner": "sre-team" }),
        },
        UpdateCase {
            label: "a new display name",
            update: json!({ "DisplayName": "Renamed Display" }),
        },
        UpdateCase {
            label: "every attribute at once",
            update: json!({
                "Description": "Everything updated",
                "Criticality": 7,
                "Owner": "new-owner@example.com",
                "DisplayName": "All Updated",
            }),
        },
        UpdateCase {
            label: "no attribute at all, which changes nothing",
            update: json!({}),
        },
    ];

    for case in cases {
        let label = case.label;
        let emulator = Emulator::new();
        let created = emulator.ok("/groups", &base).await;

        let mut request = case.update.clone();
        request["Group"] = json!(name);
        let updated = emulator.ok("/update-group", &request).await;

        for member in UPDATABLE {
            // The rule: the request's value where it carries one, the stored
            // (create-time) value where it does not.
            let expected = case
                .update
                .get(member)
                .or_else(|| base.get(member))
                .cloned()
                .unwrap_or(Value::Null);
            let answered = group_member(&updated, member);
            assert_eq!(
                expected, answered,
                "case {label:?}: expected UpdateGroup to report Group.{member} {expected}, \
                 got {answered}",
            );
        }
        // The identity of the group is not updatable: the name and the ARN it was
        // created with survive the update.
        for member in ["Name", "GroupArn"] {
            assert_eq!(
                group_member(&created, member),
                group_member(&updated, member),
                "case {label:?}: UpdateGroup must not change Group.{member}: created {}, \
                 updated {}",
                group_member(&created, member),
                group_member(&updated, member),
            );
        }

        // Read-after-write: the update reached the store, so every reader returns
        // exactly the group the update reported.
        let read = emulator.ok("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            read["Group"], updated["Group"],
            "case {label:?}: GetGroup returned {}, expected the updated group {}",
            read["Group"], updated["Group"],
        );
        let deleted = emulator.ok("/delete-group", &json!({ "Group": name })).await;
        assert_eq!(
            deleted["Group"], updated["Group"],
            "case {label:?}: DeleteGroup returned {}, expected the updated group {}",
            deleted["Group"], updated["Group"],
        );
    }
}

/// `UpdateGroup` resolves the group the same way every other operation that
/// addresses one does — by name, by the deprecated `GroupName`, or by ARN — and
/// refuses a request that names none, or one that names no stored group.
#[tokio::test]
async fn update_group_resolves_every_identifier_form_and_refuses_the_rest() {
    let emulator = Emulator::new();
    let base = update_base();
    let name = base["Name"].as_str().expect("the base names the group");
    let created = emulator.ok("/groups", &base).await;
    let arn = created["Group"]["GroupArn"]
        .as_str()
        .expect("CreateGroup returns a GroupArn")
        .to_string();

    // (label, the identifier members, the description that update applies)
    let forms = vec![
        ("the group name in Group", json!({ "Group": name }), "By name"),
        (
            "the deprecated GroupName",
            json!({ "GroupName": name }),
            "By deprecated GroupName",
        ),
        ("the group ARN", json!({ "Group": arn.clone() }), "By ARN"),
        (
            // `Group` wins over the deprecated `GroupName` when both are sent.
            "both members, Group winning",
            json!({ "Group": name, "GroupName": name }),
            "By both",
        ),
    ];
    for (label, identifier, description) in forms {
        let mut request = identifier.clone();
        request["Description"] = json!(description);
        let (status, updated) = emulator.call("/update-group", &request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected UpdateGroup {identifier} to answer 200, \
             got {status} {updated}",
        );
        assert_eq!(
            group_member(&updated, "Description").as_str(),
            Some(description),
            "case {label:?}: expected Description {description}, got {}",
            group_member(&updated, "Description"),
        );
        let read = emulator.ok("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            read["Group"], updated["Group"],
            "case {label:?}: the update must be what GetGroup returns, got {}",
            read["Group"],
        );
    }

    // (label, the whole request, the expected status and error code)
    let refused = vec![
        (
            "a name that names no stored group",
            json!({ "Group": "no-such-group", "Description": "Nope" }),
            (404, "NotFoundException"),
        ),
        (
            "an ARN whose account is not this one",
            json!({
                "Group": format!("arn:aws:resource-groups:us-east-1:111122223333:group/{name}"),
                "Description": "Nope",
            }),
            (404, "NotFoundException"),
        ),
        (
            "an ARN whose region is not the one the group was minted in",
            json!({
                "Group": format!("arn:aws:resource-groups:eu-west-1:{ACCOUNT}:group/{name}"),
                "Description": "Nope",
            }),
            (404, "NotFoundException"),
        ),
        (
            "no identifier at all",
            json!({ "Description": "Nope" }),
            (400, "BadRequestException"),
        ),
        (
            // One below the modelled `Criticality` 1..10 range.
            "criticality below the modelled range",
            json!({ "Group": name, "Criticality": 0 }),
            (400, "BadRequestException"),
        ),
        (
            // One above it.
            "criticality above the modelled range",
            json!({ "Group": name, "Criticality": 11 }),
            (400, "BadRequestException"),
        ),
        (
            // One past the modelled `Description` 0..1024 length.
            "a description past the modelled length",
            json!({ "Group": name, "Description": "d".repeat(1025) }),
            (400, "BadRequestException"),
        ),
        (
            "a description outside the modelled pattern",
            json!({ "Group": name, "Description": "no commas, please" }),
            (400, "BadRequestException"),
        ),
    ];
    let before = emulator.ok("/get-group", &json!({ "Group": name })).await;
    for (label, request, expected) in refused {
        let (status, body) = emulator.call("/update-group", &request).await;
        assert_eq!(
            (status, error_code(&body)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} {body}",
        );
        let after = emulator.ok("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            after["Group"], before["Group"],
            "case {label:?}: a refused update must leave the stored group alone: \
             was {}, now {}",
            before["Group"], after["Group"],
        );
    }
}
