// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateStack` over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * every member an `UpdateStack` request carries replaces the stored stack field, and
//!   a member it omits leaves that field alone;
//! * every `StackAttribute` named in `AttributesToDelete` clears the stored field behind
//!   it, so the member is ABSENT from the returned stack and from a later
//!   `DescribeStacks` — not present-and-empty — while the rest of the stack survives;
//! * clearing an attribute only detaches what the stack referenced, so the same value
//!   can be attached again by a later request;
//! * an unknown stack is refused with the modelled `ResourceNotFoundException` (400),
//!   and a request that both sets a field and deletes it with
//!   `InvalidParameterCombinationException` (400), leaving the stack untouched.

use serde_json::{json, Value};

use crate::stack_fleet_lifecycle::{assert_error, Emulator};

/// Members the response reports in a different shape than the request carries them
/// (`ApplicationSettings` → `ApplicationSettingsResponse`), or that the `Stack` shape
/// does not carry at all (`Tags`). They have their own assertions below, so the generic
/// comparisons skip them.
const SHAPE_SHIFTED: &[&str] = &["ApplicationSettings", "AgentAccessConfig", "Tags"];

/// One `UpdateStack` case: the stack to create, the update to send, and the members the
/// updated stack must report back exactly as the update carried them.
struct UpdateRow {
    case: &'static str,
    create: Value,
    update: Value,
    echoed: &'static [&'static str],
}

/// Update requests spanning the input class: the rows differ along the KIND of member
/// being written (a scalar string, a URL, an enum, a list of structures, a list of
/// strings, a nested structure), along whether the stack already had a value for it, and
/// along the number of entries a list carries. The last row carries nothing but `Name`.
///
/// Member names and the enum values come from
/// `smithy_query(query="operation", operation="UpdateStack")` and the `*_VALUES`
/// constants in `src/ops/types/shared_types.rs` (`ACTION_VALUES`, `PERMISSION_VALUES`,
/// `STORAGE_CONNECTOR_TYPE_VALUES`, `ACCESS_ENDPOINT_TYPE_VALUES`,
/// `PREFERRED_PROTOCOL_VALUES`, `STORAGE_CONNECTOR_AUTHENTICATION_VALUES`).
fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "description and display name replaced",
            create: json!({ "Name": "described", "Description": "before", "DisplayName": "Before" }),
            update: json!({ "Name": "described", "Description": "after", "DisplayName": "After" }),
            echoed: &["Description", "DisplayName"],
        },
        UpdateRow {
            case: "urls set on a stack that had none",
            create: json!({ "Name": "urls", "Description": "kept" }),
            update: json!({
                "Name": "urls",
                "RedirectURL": "https://example.invalid/after",
                "FeedbackURL": "https://example.invalid/feedback",
            }),
            echoed: &["RedirectURL", "FeedbackURL"],
        },
        UpdateRow {
            case: "one storage connector replaced by two others",
            create: json!({
                "Name": "connectors",
                "StorageConnectors": [{ "ConnectorType": "HOMEFOLDERS" }],
            }),
            update: json!({
                "Name": "connectors",
                "StorageConnectors": [
                    { "ConnectorType": "GOOGLE_DRIVE", "Domains": ["example.invalid"] },
                    { "ConnectorType": "ONE_DRIVE", "Domains": ["one.example.invalid"] },
                ],
            }),
            echoed: &["StorageConnectors"],
        },
        UpdateRow {
            case: "user settings replaced and grown",
            create: json!({
                "Name": "settings",
                "UserSettings": [{ "Action": "FILE_UPLOAD", "Permission": "ENABLED" }],
            }),
            update: json!({
                "Name": "settings",
                "UserSettings": [
                    { "Action": "FILE_UPLOAD", "Permission": "DISABLED" },
                    { "Action": "PRINTING_TO_LOCAL_DEVICE", "Permission": "DISABLED" },
                    { "Action": "CLIPBOARD_COPY_FROM_LOCAL_DEVICE", "Permission": "ENABLED" },
                ],
            }),
            echoed: &["UserSettings"],
        },
        UpdateRow {
            // `AccessEndpointList` is @length 1..4 and `AccessEndpointType` models the
            // single value STREAMING, so re-pointing the stack at another endpoint id is
            // what varies here.
            case: "access endpoint re-pointed at another VPC endpoint",
            create: json!({
                "Name": "endpoints",
                "AccessEndpoints": [{ "EndpointType": "STREAMING", "VpceId": "vpce-0a1b2c3d4e5f60000" }],
            }),
            update: json!({
                "Name": "endpoints",
                "AccessEndpoints": [{ "EndpointType": "STREAMING", "VpceId": "vpce-0a1b2c3d4e5f61111" }],
            }),
            echoed: &["AccessEndpoints"],
        },
        UpdateRow {
            case: "embed host domains grown from one to three",
            create: json!({ "Name": "embedded", "EmbedHostDomains": ["a.example.invalid"] }),
            update: json!({
                "Name": "embedded",
                "EmbedHostDomains": [
                    "a.example.invalid",
                    "b.example.invalid",
                    "c.example.invalid",
                ],
            }),
            echoed: &["EmbedHostDomains"],
        },
        UpdateRow {
            case: "streaming protocol switched between the two modelled values",
            create: json!({
                "Name": "protocol",
                "StreamingExperienceSettings": { "PreferredProtocol": "TCP" },
            }),
            update: json!({
                "Name": "protocol",
                "StreamingExperienceSettings": { "PreferredProtocol": "UDP" },
            }),
            echoed: &["StreamingExperienceSettings"],
        },
        UpdateRow {
            case: "nested content redirection configuration added",
            create: json!({ "Name": "redirection", "Description": "kept" }),
            update: json!({
                "Name": "redirection",
                "ContentRedirection": {
                    "HostToClient": {
                        "Enabled": true,
                        "AllowedUrls": ["https://allowed.example.invalid/"],
                        "DeniedUrls": ["https://denied.example.invalid/"],
                    },
                },
            }),
            echoed: &["ContentRedirection"],
        },
        UpdateRow {
            case: "storage connector authentication switched between the two modelled values",
            create: json!({
                "Name": "authentication",
                "StorageConnectorAuthentication": "REUSE_CREDENTIALS",
            }),
            update: json!({
                "Name": "authentication",
                "StorageConnectorAuthentication": "FORCE_REAUTHENTICATION",
            }),
            echoed: &["StorageConnectorAuthentication"],
        },
        UpdateRow {
            case: "name only: every stored field is left alone",
            create: json!({
                "Name": "untouched",
                "Description": "kept",
                "RedirectURL": "https://example.invalid/kept",
                "EmbedHostDomains": ["kept.example.invalid"],
                "StorageConnectors": [{ "ConnectorType": "HOMEFOLDERS" }],
            }),
            update: json!({ "Name": "untouched" }),
            echoed: &[],
        },
    ]
}

#[tokio::test]
async fn update_stack_applies_the_members_it_is_given_and_leaves_the_rest() {
    let emulator = Emulator::new();

    for row in update_rows() {
        let UpdateRow {
            case,
            create,
            update,
            echoed,
        } = row;
        let name = create["Name"].as_str().expect("every row names a stack");

        emulator.ok(case, "CreateStack", create.clone()).await;
        let updated = emulator.ok(case, "UpdateStack", update.clone()).await;
        let stack = &updated["Stack"];

        assert_eq!(
            stack["Name"].as_str(),
            Some(name),
            "case {case:?}: UpdateStack must report the stack it updated, got {stack}"
        );
        for member in echoed {
            assert_eq!(
                &stack[*member], &update[*member],
                "case {case:?}: UpdateStack must store and report {member}, got {stack}"
            );
        }

        // A member the update does not carry keeps the value the create persisted.
        for (member, created_value) in create
            .as_object()
            .expect("a create request is an object")
            .iter()
            .filter(|(member, _)| member.as_str() != "Name")
            .filter(|(member, _)| !SHAPE_SHIFTED.contains(&member.as_str()))
            .filter(|(member, _)| update.get(member.as_str()).is_none())
        {
            assert_eq!(
                &stack[member.as_str()], created_value,
                "case {case:?}: UpdateStack must leave {member} alone, got {stack}"
            );
        }

        // ROUND TRIP: the reader answers from the updated record, not from the create.
        let described = emulator
            .ok(case, "DescribeStacks", json!({ "Names": [name] }))
            .await;
        assert_eq!(
            described["Stacks"][0], *stack,
            "case {case:?}: DescribeStacks must report the updated record, got {described}"
        );
    }
}

#[tokio::test]
async fn update_stack_reports_application_settings_in_their_response_form() {
    let emulator = Emulator::new();
    let case = "persistent application settings replaced";

    emulator
        .ok(
            case,
            "CreateStack",
            json!({
                "Name": "app-settings",
                "ApplicationSettings": { "Enabled": true, "SettingsGroup": "group-a" },
            }),
        )
        .await;

    let updated = emulator
        .ok(
            case,
            "UpdateStack",
            json!({
                "Name": "app-settings",
                "ApplicationSettings": { "Enabled": true, "SettingsGroup": "group-b" },
            }),
        )
        .await;
    let settings = &updated["Stack"]["ApplicationSettings"];
    assert_eq!(
        settings["Enabled"],
        json!(true),
        "case {case:?}: ApplicationSettings.Enabled must round-trip, got {settings}"
    );
    assert_eq!(
        settings["SettingsGroup"],
        json!("group-b"),
        "case {case:?}: the updated SettingsGroup must be stored, got {settings}"
    );

    let described = emulator
        .ok(case, "DescribeStacks", json!({ "Names": ["app-settings"] }))
        .await;
    assert_eq!(
        described["Stacks"][0]["ApplicationSettings"], *settings,
        "case {case:?}: DescribeStacks must report the updated settings, got {described}"
    );
}

/// One `AttributesToDelete` case: the stack to create, the attribute to delete, and the
/// `Stack` member that must be absent afterwards.
struct DeleteRow {
    case: &'static str,
    create: Value,
    attribute: &'static str,
    member: &'static str,
}

/// Every `StackAttribute` that names a member of the `Stack` shape, each on a stack that
/// carries a value for it. The values are `StackAttribute`'s
/// (`smithy_query(query="shape", shape="StackAttribute")`, kept as
/// `STACK_ATTRIBUTE_VALUES` in `src/ops/types/shared_types.rs`); the three
/// `STORAGE_CONNECTOR_<type>` values remove ONE connector rather than a whole member and
/// are covered by their own case below, and THEME_NAME / IAM_ROLE_ARN name no `Stack`
/// member at all.
fn delete_rows() -> Vec<DeleteRow> {
    vec![
        DeleteRow {
            case: "access endpoints detached",
            create: json!({
                "Name": "del-access-endpoints",
                "Description": "kept",
                "AccessEndpoints": [{ "EndpointType": "STREAMING", "VpceId": "vpce-0a1b2c3d4e5f62222" }],
            }),
            attribute: "ACCESS_ENDPOINTS",
            member: "AccessEndpoints",
        },
        DeleteRow {
            case: "embed host domains dropped",
            create: json!({
                "Name": "del-embed-hosts",
                "Description": "kept",
                "EmbedHostDomains": ["a.example.invalid", "b.example.invalid"],
            }),
            attribute: "EMBED_HOST_DOMAINS",
            member: "EmbedHostDomains",
        },
        DeleteRow {
            case: "user settings dropped",
            create: json!({
                "Name": "del-user-settings",
                "Description": "kept",
                "UserSettings": [{ "Action": "FILE_DOWNLOAD", "Permission": "DISABLED" }],
            }),
            attribute: "USER_SETTINGS",
            member: "UserSettings",
        },
        DeleteRow {
            case: "every storage connector dropped",
            create: json!({
                "Name": "del-storage-connectors",
                "Description": "kept",
                "StorageConnectors": [
                    { "ConnectorType": "HOMEFOLDERS" },
                    { "ConnectorType": "ONE_DRIVE", "Domains": ["one.example.invalid"] },
                ],
            }),
            attribute: "STORAGE_CONNECTORS",
            member: "StorageConnectors",
        },
        DeleteRow {
            case: "redirect url cleared",
            create: json!({
                "Name": "del-redirect-url",
                "Description": "kept",
                "RedirectURL": "https://example.invalid/after",
            }),
            attribute: "REDIRECT_URL",
            member: "RedirectURL",
        },
        DeleteRow {
            case: "feedback url cleared",
            create: json!({
                "Name": "del-feedback-url",
                "Description": "kept",
                "FeedbackURL": "https://example.invalid/feedback",
            }),
            attribute: "FEEDBACK_URL",
            member: "FeedbackURL",
        },
        DeleteRow {
            case: "streaming experience settings cleared",
            create: json!({
                "Name": "del-streaming-experience",
                "Description": "kept",
                "StreamingExperienceSettings": { "PreferredProtocol": "UDP" },
            }),
            attribute: "STREAMING_EXPERIENCE_SETTINGS",
            member: "StreamingExperienceSettings",
        },
        DeleteRow {
            case: "content redirection cleared",
            create: json!({
                "Name": "del-content-redirection",
                "Description": "kept",
                "ContentRedirection": { "HostToClient": { "Enabled": true } },
            }),
            attribute: "CONTENT_REDIRECTION",
            member: "ContentRedirection",
        },
        DeleteRow {
            case: "storage connector authentication cleared",
            create: json!({
                "Name": "del-connector-authentication",
                "Description": "kept",
                "StorageConnectorAuthentication": "FORCE_REAUTHENTICATION",
            }),
            attribute: "STORAGE_CONNECTOR_AUTHENTICATION",
            member: "StorageConnectorAuthentication",
        },
        DeleteRow {
            case: "agent access configuration removed",
            create: json!({
                "Name": "del-agent-access",
                "Description": "kept",
                "AgentAccessConfig": {
                    "Settings": [{ "AgentAction": "COMPUTER_INPUT", "Permission": "ENABLED" }],
                    "ScreenResolution": "W_1280xH_720",
                    "ScreenImageFormat": "PNG",
                },
            }),
            attribute: "AGENT_ACCESS_CONFIG",
            member: "AgentAccessConfig",
        },
    ]
}

#[tokio::test]
async fn attributes_to_delete_removes_the_key_from_the_stack() {
    let emulator = Emulator::new();

    for row in delete_rows() {
        let DeleteRow {
            case,
            create,
            attribute,
            member,
        } = row;
        let name = create["Name"].as_str().expect("every row names a stack").to_string();

        let created = emulator.ok(case, "CreateStack", create).await;
        assert!(
            created["Stack"].get(member).is_some(),
            "case {case:?}: the created stack must carry {member} for the delete to be \
             observable, got {created}"
        );

        let updated = emulator
            .ok(
                case,
                "UpdateStack",
                json!({ "Name": name, "AttributesToDelete": [attribute] }),
            )
            .await;
        let stack = &updated["Stack"];
        assert!(
            stack.get(member).is_none(),
            "case {case:?}: AttributesToDelete=[{attribute}] must leave no {member} key \
             (absent, not empty), got {stack}"
        );
        assert_eq!(
            stack["Description"],
            json!("kept"),
            "case {case:?}: deleting {attribute} must not touch the rest of the stack, \
             got {stack}"
        );

        let described = emulator
            .ok(case, "DescribeStacks", json!({ "Names": [name] }))
            .await;
        let read_back = &described["Stacks"][0];
        assert!(
            read_back.get(member).is_none(),
            "case {case:?}: DescribeStacks must report no {member} key either, got \
             {described}"
        );
        assert_eq!(
            read_back, stack,
            "case {case:?}: DescribeStacks must report the updated record, got {described}"
        );
    }
}

/// The connector types the three `STORAGE_CONNECTOR_<type>` attributes name, from
/// `STORAGE_CONNECTOR_TYPE_VALUES` in `src/ops/types/shared_types.rs`.
const CONNECTOR_TYPES: &[(&str, &str)] = &[
    ("STORAGE_CONNECTOR_HOMEFOLDERS", "HOMEFOLDERS"),
    ("STORAGE_CONNECTOR_GOOGLE_DRIVE", "GOOGLE_DRIVE"),
    ("STORAGE_CONNECTOR_ONE_DRIVE", "ONE_DRIVE"),
];

/// The connector types a stack reports, in order.
fn connector_types(stack: &Value) -> Vec<String> {
    stack["StorageConnectors"]
        .as_array()
        .map(|connectors| {
            connectors
                .iter()
                .map(|connector| connector["ConnectorType"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn deleting_one_connector_attribute_leaves_the_other_connectors_enabled() {
    let emulator = Emulator::new();

    for (attribute, connector_type) in CONNECTOR_TYPES {
        let case = *attribute;
        let name = format!("one-connector-{}", connector_type.to_lowercase());
        emulator
            .ok(
                case,
                "CreateStack",
                json!({
                    "Name": name,
                    "StorageConnectors": [
                        { "ConnectorType": "HOMEFOLDERS" },
                        { "ConnectorType": "GOOGLE_DRIVE" },
                        { "ConnectorType": "ONE_DRIVE" },
                    ],
                }),
            )
            .await;

        let updated = emulator
            .ok(
                case,
                "UpdateStack",
                json!({ "Name": name, "AttributesToDelete": [attribute] }),
            )
            .await;
        let expected: Vec<String> = CONNECTOR_TYPES
            .iter()
            .map(|(_, other)| (*other).to_string())
            .filter(|other| other != connector_type)
            .collect();
        let actual = connector_types(&updated["Stack"]);
        assert_eq!(
            actual, expected,
            "case {case:?}: deleting {attribute} must drop only the {connector_type} \
             connector, got {actual:?}"
        );
    }

    // Naming all three in one request leaves no connector, so the member is absent.
    let case = "every connector type named at once";
    emulator
        .ok(
            case,
            "CreateStack",
            json!({
                "Name": "all-connectors",
                "StorageConnectors": [
                    { "ConnectorType": "HOMEFOLDERS" },
                    { "ConnectorType": "GOOGLE_DRIVE" },
                    { "ConnectorType": "ONE_DRIVE" },
                ],
            }),
        )
        .await;
    let updated = emulator
        .ok(
            case,
            "UpdateStack",
            json!({
                "Name": "all-connectors",
                "AttributesToDelete": CONNECTOR_TYPES
                    .iter()
                    .map(|(attribute, _)| *attribute)
                    .collect::<Vec<_>>(),
            }),
        )
        .await;
    assert!(
        updated["Stack"].get("StorageConnectors").is_none(),
        "case {case:?}: a stack with no connector left must report no StorageConnectors \
         key, got {updated}"
    );
}

#[tokio::test]
async fn an_attribute_naming_no_stack_member_is_accepted_and_changes_nothing() {
    let emulator = Emulator::new();
    // THEME_NAME names the stack's theme (a resource of its own) and IAM_ROLE_ARN names
    // a field the `Stack` shape does not model: both are modelled `StackAttribute`
    // values, so the request is valid and the stack itself is unchanged.
    for attribute in ["THEME_NAME", "IAM_ROLE_ARN"] {
        let case = attribute;
        let name = format!("no-member-{}", attribute.to_lowercase());
        let created = emulator
            .ok(
                case,
                "CreateStack",
                json!({
                    "Name": name,
                    "Description": "kept",
                    "AccessEndpoints": [{ "EndpointType": "STREAMING", "VpceId": "vpce-0a1b2c3d4e5f63333" }],
                }),
            )
            .await;
        let updated = emulator
            .ok(
                case,
                "UpdateStack",
                json!({ "Name": name, "AttributesToDelete": [attribute] }),
            )
            .await;
        assert_eq!(
            updated["Stack"], created["Stack"],
            "case {case:?}: deleting {attribute} must leave the stack as it was, got \
             {updated}"
        );
    }
}

#[tokio::test]
async fn a_detached_access_endpoint_can_be_attached_again() {
    let emulator = Emulator::new();
    let case = "detach then re-attach the same endpoint";
    let endpoints = json!([{ "EndpointType": "STREAMING", "VpceId": "vpce-0a1b2c3d4e5f64444" }]);

    emulator
        .ok(
            case,
            "CreateStack",
            json!({ "Name": "reattach", "AccessEndpoints": endpoints }),
        )
        .await;
    let detached = emulator
        .ok(
            case,
            "UpdateStack",
            json!({ "Name": "reattach", "AttributesToDelete": ["ACCESS_ENDPOINTS"] }),
        )
        .await;
    assert!(
        detached["Stack"].get("AccessEndpoints").is_none(),
        "case {case:?}: the detach must leave no AccessEndpoints key, got {detached}"
    );

    let reattached = emulator
        .ok(
            case,
            "UpdateStack",
            json!({ "Name": "reattach", "AccessEndpoints": endpoints }),
        )
        .await;
    assert_eq!(
        reattached["Stack"]["AccessEndpoints"], endpoints,
        "case {case:?}: the same endpoint must be attachable again, got {reattached}"
    );

    let described = emulator
        .ok(case, "DescribeStacks", json!({ "Names": ["reattach"] }))
        .await;
    assert_eq!(
        described["Stacks"][0]["AccessEndpoints"], endpoints,
        "case {case:?}: DescribeStacks must report the re-attached endpoint, got \
         {described}"
    );
}

#[tokio::test]
async fn update_stack_merges_a_partial_agent_access_configuration() {
    let emulator = Emulator::new();
    let case = "partial agent access update";

    emulator
        .ok(
            case,
            "CreateStack",
            json!({
                "Name": "agent",
                "AgentAccessConfig": {
                    "Settings": [{ "AgentAction": "COMPUTER_INPUT", "Permission": "ENABLED" }],
                    "ScreenResolution": "W_1280xH_720",
                    "ScreenImageFormat": "PNG",
                },
            }),
        )
        .await;

    // Only `ScreenImageFormat` is supplied: it changes and every other member keeps the
    // stored value.
    let updated = emulator
        .ok(
            case,
            "UpdateStack",
            json!({ "Name": "agent", "AgentAccessConfig": { "ScreenImageFormat": "JPEG" } }),
        )
        .await;
    let config = &updated["Stack"]["AgentAccessConfig"];
    assert_eq!(
        config["ScreenImageFormat"],
        json!("JPEG"),
        "case {case:?}: the supplied member must be stored, got {config}"
    );
    assert_eq!(
        config["ScreenResolution"],
        json!("W_1280xH_720"),
        "case {case:?}: an omitted member must keep its stored value, got {config}"
    );
    assert_eq!(
        config["Settings"],
        json!([{ "AgentAction": "COMPUTER_INPUT", "Permission": "ENABLED" }]),
        "case {case:?}: an omitted list must keep its stored entries, got {config}"
    );

    // And the list itself can be replaced on its own.
    let updated = emulator
        .ok(
            case,
            "UpdateStack",
            json!({
                "Name": "agent",
                "AgentAccessConfig": {
                    "Settings": [{ "AgentAction": "COMPUTER_VISION", "Permission": "DISABLED" }],
                },
            }),
        )
        .await;
    let config = &updated["Stack"]["AgentAccessConfig"];
    assert_eq!(
        config["Settings"],
        json!([{ "AgentAction": "COMPUTER_VISION", "Permission": "DISABLED" }]),
        "case {case:?}: the supplied settings must replace the stored ones, got {config}"
    );
    assert_eq!(
        config["ScreenImageFormat"],
        json!("JPEG"),
        "case {case:?}: the earlier update must still be stored, got {config}"
    );
}

#[tokio::test]
async fn update_stack_refuses_an_unknown_stack_with_resource_not_found() {
    let emulator = Emulator::new();

    emulator
        .ok("setup", "CreateStack", json!({ "Name": "deleted-later" }))
        .await;
    emulator
        .ok("setup", "DeleteStack", json!({ "Name": "deleted-later" }))
        .await;

    // Rows: a name nothing ever created, a name whose stack was deleted, and a name that
    // differs from a stored one only in case.
    emulator
        .ok("setup", "CreateStack", json!({ "Name": "CasedStack" }))
        .await;
    for (case, name) in [
        ("never created", "no-such-stack"),
        ("deleted", "deleted-later"),
        ("differs in case from a stored stack", "casedstack"),
    ] {
        assert_error(
            &emulator,
            case,
            "UpdateStack",
            json!({ "Name": name, "Description": "anything" }),
            400,
            "ResourceNotFoundException",
        )
        .await;
    }
}

#[tokio::test]
async fn setting_and_deleting_the_same_field_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "setup",
            "CreateStack",
            json!({
                "Name": "conflicting",
                "RedirectURL": "https://example.invalid/before",
                "UserSettings": [{ "Action": "FILE_UPLOAD", "Permission": "ENABLED" }],
                "StorageConnectors": [{ "ConnectorType": "HOMEFOLDERS" }],
                "AccessEndpoints": [{ "EndpointType": "STREAMING", "VpceId": "vpce-0a1b2c3d4e5f65555" }],
            }),
        )
        .await;

    // One row per kind of member: a scalar, a list of structures, a list the attribute
    // names per entry, and the endpoint list the issue's trace exercises.
    for (case, attribute, member, value) in [
        (
            "redirect url",
            "REDIRECT_URL",
            "RedirectURL",
            json!("https://example.invalid/after"),
        ),
        (
            "user settings",
            "USER_SETTINGS",
            "UserSettings",
            json!([{ "Action": "FILE_DOWNLOAD", "Permission": "DISABLED" }]),
        ),
        (
            "one storage connector type",
            "STORAGE_CONNECTOR_HOMEFOLDERS",
            "StorageConnectors",
            json!([{ "ConnectorType": "GOOGLE_DRIVE" }]),
        ),
        (
            "access endpoints",
            "ACCESS_ENDPOINTS",
            "AccessEndpoints",
            json!([{ "EndpointType": "STREAMING", "VpceId": "vpce-0a1b2c3d4e5f66666" }]),
        ),
    ] {
        assert_error(
            &emulator,
            case,
            "UpdateStack",
            json!({
                "Name": "conflicting",
                member: value,
                "AttributesToDelete": [attribute],
            }),
            400,
            "InvalidParameterCombinationException",
        )
        .await;
    }

    let described = emulator
        .ok("after the refusals", "DescribeStacks", json!({ "Names": ["conflicting"] }))
        .await;
    assert_eq!(
        described["Stacks"][0], created["Stack"],
        "a refused UpdateStack must store nothing, got {described}"
    );
}

#[tokio::test]
async fn update_stack_refuses_an_unknown_attribute_and_an_unfounded_agent_config() {
    let emulator = Emulator::new();
    emulator
        .ok("setup", "CreateStack", json!({ "Name": "refusals" }))
        .await;

    // Not a `StackAttribute` value, so there is no stored field it could name.
    assert_error(
        &emulator,
        "unknown stack attribute",
        "UpdateStack",
        json!({ "Name": "refusals", "AttributesToDelete": ["NOT_A_STACK_ATTRIBUTE"] }),
        400,
        "ValidationException",
    )
    .await;

    // A partial agent access update on a stack with no stored configuration: the members
    // `AgentAccessConfig` requires are missing and cannot be invented.
    assert_error(
        &emulator,
        "partial agent access config on a stack that has none",
        "UpdateStack",
        json!({
            "Name": "refusals",
            "AgentAccessConfig": { "ScreenImageFormat": "PNG" },
        }),
        400,
        "InvalidParameterCombinationException",
    )
    .await;

    // The same request succeeds once every required member is supplied.
    let updated = emulator
        .ok(
            "complete agent access config",
            "UpdateStack",
            json!({
                "Name": "refusals",
                "AgentAccessConfig": {
                    "Settings": [{ "AgentAction": "FORWARD_MCP_TOOLS", "Permission": "ENABLED" }],
                    "ScreenResolution": "W_1280xH_720",
                    "ScreenImageFormat": "PNG",
                },
            }),
        )
        .await;
    assert_eq!(
        updated["Stack"]["AgentAccessConfig"]["ScreenImageFormat"],
        json!("PNG"),
        "a complete AgentAccessConfig must be stored, got {updated}"
    );
}
