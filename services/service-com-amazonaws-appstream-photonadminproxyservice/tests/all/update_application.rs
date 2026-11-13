// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateApplication` over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * every member an `UpdateApplication` request carries replaces the stored application
//!   field, a member it omits leaves that field alone, and the identity the create minted
//!   (`Name`, `Arn`, `CreatedTime`) is never rewritten;
//! * every `ApplicationAttribute` named in `AttributesToDelete` clears the stored field
//!   behind it, so the member is ABSENT from the returned application and from a later
//!   `DescribeApplications` — not present-and-empty — while the rest of the application
//!   survives, and the same value can be set again afterwards;
//! * the update reports the record it stored: a `DescribeApplications` answers with the
//!   updated application rather than with what the create persisted;
//! * `Metadata` follows the application's own attributes rather than standing on its own:
//!   an update that sets one adds its entry and one that deletes one drops it
//!   (`tests/all/application_update.rs` asserts that map over the whole attribute class);
//! * a `Name` the account holds no application under is refused with
//!   `ResourceNotFoundException` (404) carrying AWS's sentence for that class, as is an
//!   `AppBlockArn` that addresses no app block this account holds, and both leave the
//!   stored application exactly as it was.

use serde_json::{json, Value};

use crate::applications::{create_app_block, expected_metadata};
use crate::refusal_messages::{not_found_sentence, refusal};
use crate::stack_fleet_lifecycle::{assert_error, Emulator};

/// The account every emulated resource belongs to.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the harness signs for, which is the region every ARN below carries.
const REGION: &str = "us-east-1";

/// The app block the applications below are created against, and the second one a row
/// re-points an application at.
const APP_BLOCK: &str = "update-block";
const OTHER_APP_BLOCK: &str = "other-block";

/// The resource word AWS's refusals use for an application, read from the crate so the
/// test tracks the constant rather than a copy of its value.
const APPLICATION_KIND: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::APPLICATION_KIND;

/// The resource word AWS's refusal uses for the app block an application's binaries come
/// from.
const APP_BLOCK_ARN_KIND: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::APP_BLOCK_ARN_KIND;

/// One deletable attribute: the `ApplicationAttribute` value, the `Application` member
/// it clears, and a value to store for that member first.
type Deletable = (&'static str, &'static str, &'static str);

/// The two `ApplicationAttribute` values, with the `Application` member each one clears
/// and a value to store for it first. The values are `ApplicationAttribute`'s
/// (`smithy_query(query="shape", shape="ApplicationAttribute")`, kept as
/// `APPLICATION_ATTRIBUTE_VALUES` in `src/ops/types/shared_types.rs`).
const DELETABLE_ATTRIBUTES: &[Deletable] = &[
    (
        "LAUNCH_PARAMETERS",
        "LaunchParameters",
        "--kiosk --locale=en_GB",
    ),
    ("WORKING_DIRECTORY", "WorkingDirectory", "/opt/app"),
];

/// The `CreateApplication` request for `name` against `app_block_arn`, carrying the
/// members a row then updates.
fn create_request(name: &str, app_block_arn: &str) -> Value {
    json!({
        "Name": name,
        "AppBlockArn": app_block_arn,
        "IconS3Location": { "S3Bucket": "app-icons", "S3Key": "icons/before.png" },
        "LaunchPath": "C:\\Program Files\\App\\app.exe",
        "Platforms": ["WINDOWS_SERVER_2019"],
        "InstanceFamilies": ["GENERAL_PURPOSE"],
        "DisplayName": "Before",
        "Description": "before",
    })
}

/// The application a call reported, minus the icon URL.
///
/// `Application.IconURL` is a time-limited address signed per render (see
/// `crate::applications`), so it is asserted on its own rather than compared between two
/// calls.
fn without_icon_url(application: &Value) -> Value {
    let mut copy = application.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("IconURL");
    }
    copy
}

/// The application `DescribeApplications` reports for `name`.
async fn describe_one(emulator: &Emulator, case: &str, name: &str) -> Value {
    let arn = format!("arn:aws:appstream:{REGION}:{ACCOUNT}:application/{name}");
    let payload = emulator
        .ok(case, "DescribeApplications", json!({ "Arns": [arn] }))
        .await;
    payload["Applications"][0].clone()
}

/// One `UpdateApplication` case: the application to create, the update to send, and the
/// members the updated application must report back exactly as the update carried them.
struct UpdateRow {
    case: &'static str,
    name: &'static str,
    update: Value,
    echoed: &'static [&'static str],
}

/// Update requests spanning the input class: the rows differ along the KIND of member
/// being written (a scalar string, a path, a nested structure, a reference to another
/// resource), along whether the application already had a value for it, and the last row
/// carries nothing but `Name`.
///
/// Member names come from
/// `smithy_query(query="operation", operation="UpdateApplication")`.
fn update_rows(other_app_block_arn: &str) -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "display name and description replaced",
            name: "described",
            update: json!({
                "Name": "described",
                "DisplayName": "After",
                "Description": "after",
            }),
            echoed: &["DisplayName", "Description"],
        },
        UpdateRow {
            case: "launch details set on an application that had none",
            name: "launch.details-1",
            update: json!({
                "Name": "launch.details-1",
                "LaunchPath": "/opt/app/bin/app",
                "LaunchParameters": "--kiosk",
                "WorkingDirectory": "/opt/app",
            }),
            echoed: &["LaunchPath", "LaunchParameters", "WorkingDirectory"],
        },
        UpdateRow {
            case: "the icon re-pointed at another S3 object",
            name: "icon_moved",
            update: json!({
                "Name": "icon_moved",
                "IconS3Location": { "S3Bucket": "other-icons", "S3Key": "icons/after.svg" },
            }),
            echoed: &["IconS3Location"],
        },
        UpdateRow {
            case: "re-pointed at another app block",
            name: "rebound",
            update: json!({ "Name": "rebound", "AppBlockArn": other_app_block_arn }),
            echoed: &["AppBlockArn"],
        },
        UpdateRow {
            case: "nothing but the name",
            name: "untouched",
            update: json!({ "Name": "untouched" }),
            echoed: &[],
        },
    ]
}

#[tokio::test]
async fn update_application_applies_the_members_it_is_given_and_leaves_the_rest() {
    let emulator = Emulator::new();
    let app_block_arn = create_app_block(&emulator, "setup", APP_BLOCK).await;
    let other_app_block_arn = create_app_block(&emulator, "setup", OTHER_APP_BLOCK).await;

    for row in update_rows(&other_app_block_arn) {
        let UpdateRow {
            case,
            name,
            update,
            echoed,
        } = row;

        let created = emulator
            .ok(case, "CreateApplication", create_request(name, &app_block_arn))
            .await;
        let before = created["Application"].clone();

        let updated = emulator.ok(case, "UpdateApplication", update.clone()).await;
        let after = updated["Application"].clone();

        for member in echoed {
            assert_eq!(
                &after[*member], &update[*member],
                "case {case:?}: UpdateApplication must store and report {member}, got {after}"
            );
        }

        // Every other member keeps what the create persisted — the identity the create
        // minted (`Name`, `Arn`, `CreatedTime`) included. `Metadata` is not an
        // independent member: it reports the application's own attributes, so an update
        // that sets one of those moves the map with it; it is asserted on its own below.
        let moved_icon = echoed.contains(&"IconS3Location");
        let moved_attributes = echoed
            .iter()
            .any(|member| ["LaunchParameters", "WorkingDirectory"].contains(member));
        let before_members = before.as_object().expect("an application is an object");
        for (member, created_value) in before_members {
            if echoed.contains(&member.as_str())
                || (moved_icon && member == "IconURL")
                || (moved_attributes && member == "Metadata")
            {
                continue;
            }
            assert_eq!(
                &after[member.as_str()], created_value,
                "case {case:?}: UpdateApplication must leave {member} alone, got {after}"
            );
        }

        // The attribute map reports the attributes the application has AFTER the update,
        // keyed by the modelled `ApplicationAttribute` names.
        assert_eq!(
            after["Metadata"],
            expected_metadata(
                after.get("LaunchParameters").and_then(Value::as_str),
                after.get("WorkingDirectory").and_then(Value::as_str),
            ),
            "case {case:?}: Metadata must report the application's own attributes, got {after}"
        );
        // An update that deletes no attribute reports exactly the members the record had,
        // plus any the update set for the first time.
        let after_members: std::collections::BTreeSet<String> = after
            .as_object()
            .expect("an application is an object")
            .keys()
            .cloned()
            .collect();
        let mut expected_members: std::collections::BTreeSet<String> =
            before_members.keys().cloned().collect();
        expected_members.extend(echoed.iter().map(|member| (*member).to_string()));
        assert_eq!(
            after_members, expected_members,
            "case {case:?}: an update that deletes no attribute must report the record's \
             members and the ones it set, got {after}"
        );
        if moved_icon {
            let icon_url = after["IconURL"].as_str().unwrap_or_default().to_string();
            assert!(
                icon_url.contains("other-icons"),
                "case {case:?}: the icon URL must address the bucket the update named, got \
                 {icon_url:?}"
            );
        }

        // ROUND TRIP: the reader answers from the updated record, not from the create.
        let described = describe_one(&emulator, case, name).await;
        assert_eq!(
            without_icon_url(&described),
            without_icon_url(&after),
            "case {case:?}: DescribeApplications must report the updated record, got \
             {described}"
        );
    }
}

#[tokio::test]
async fn attributes_to_delete_removes_the_key_from_the_application() {
    let emulator = Emulator::new();
    let app_block_arn = create_app_block(&emulator, "setup", APP_BLOCK).await;

    // Rows: each attribute on its own, then both in one request.
    let mut cases: Vec<(&str, Vec<Deletable>)> = DELETABLE_ATTRIBUTES
        .iter()
        .map(|entry| (entry.0, vec![*entry]))
        .collect();
    cases.push(("both attributes at once", DELETABLE_ATTRIBUTES.to_vec()));

    for (case, deleted) in cases {
        let name = format!("del-{}", case.to_lowercase().replace([' ', '_'], "-"));
        let mut create = create_request(&name, &app_block_arn);
        for (_, member, value) in DELETABLE_ATTRIBUTES {
            create[*member] = json!(value);
        }
        let created = emulator.ok(case, "CreateApplication", create).await;
        for (_, member, _) in DELETABLE_ATTRIBUTES {
            assert!(
                created["Application"].get(*member).is_some(),
                "case {case:?}: the created application must carry {member} for the \
                 delete to be observable, got {created}"
            );
        }

        let attributes: Vec<&str> = deleted.iter().map(|(attribute, _, _)| *attribute).collect();
        let updated = emulator
            .ok(
                case,
                "UpdateApplication",
                json!({ "Name": name, "AttributesToDelete": attributes }),
            )
            .await;
        let after = updated["Application"].clone();

        for (attribute, member, _) in &deleted {
            assert!(
                after.get(*member).is_none(),
                "case {case:?}: AttributesToDelete=[{attribute}] must leave no {member} \
                 key (absent, not empty), got {after}"
            );
        }
        // A member no deleted attribute names keeps its stored value.
        for (_, member, value) in DELETABLE_ATTRIBUTES
            .iter()
            .filter(|(attribute, _, _)| !attributes.contains(attribute))
        {
            assert_eq!(
                after[*member],
                json!(value),
                "case {case:?}: deleting {attributes:?} must not touch {member}, got {after}"
            );
        }
        assert_eq!(
            after["Description"],
            json!("before"),
            "case {case:?}: deleting {attributes:?} must not touch the rest of the \
             application, got {after}"
        );

        let described = describe_one(&emulator, case, &name).await;
        for (_, member, _) in &deleted {
            assert!(
                described.get(*member).is_none(),
                "case {case:?}: DescribeApplications must report no {member} key either, \
                 got {described}"
            );
        }
        assert_eq!(
            without_icon_url(&described),
            without_icon_url(&after),
            "case {case:?}: DescribeApplications must report the updated record, got \
             {described}"
        );
    }
}

#[tokio::test]
async fn a_cleared_attribute_can_be_set_again() {
    let emulator = Emulator::new();
    let app_block_arn = create_app_block(&emulator, "setup", APP_BLOCK).await;

    for (attribute, member, value) in DELETABLE_ATTRIBUTES {
        let case = *attribute;
        let name = format!("reset-{}", member.to_lowercase());
        let mut create = create_request(&name, &app_block_arn);
        create[*member] = json!(value);
        emulator.ok(case, "CreateApplication", create).await;

        let cleared = emulator
            .ok(
                case,
                "UpdateApplication",
                json!({ "Name": name, "AttributesToDelete": [attribute] }),
            )
            .await;
        assert!(
            cleared["Application"].get(*member).is_none(),
            "case {case:?}: the clear must leave no {member} key, got {cleared}"
        );

        let mut set_request = json!({ "Name": name });
        set_request[*member] = json!(value);
        let set_again = emulator.ok(case, "UpdateApplication", set_request).await;
        assert_eq!(
            set_again["Application"][*member],
            json!(value),
            "case {case:?}: the same {member} must be settable again, got {set_again}"
        );

        let described = describe_one(&emulator, case, &name).await;
        assert_eq!(
            described[*member],
            json!(value),
            "case {case:?}: DescribeApplications must report the value that was set \
             again, got {described}"
        );
    }
}

/// Names spanning the class: the rows differ along length, character classes and case.
/// The last is the longest name a create admits — the `Name` member's pattern is
/// `^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$`
/// (`smithy_query(query="operation", operation="UpdateApplication")`), so 101 characters.
fn absent_names() -> Vec<String> {
    vec![
        "absent".to_string(),
        "Mixed.Case_name-9".to_string(),
        "0123456789".to_string(),
        format!("z{}", "9".repeat(100)),
    ]
}

#[tokio::test]
async fn update_application_refuses_an_unknown_application_with_resource_not_found() {
    let emulator = Emulator::new();
    let app_block_arn = create_app_block(&emulator, "setup", APP_BLOCK).await;

    // A stored application whose name differs only in case, and one that was deleted:
    // neither is addressed by the names below.
    emulator
        .ok(
            "setup",
            "CreateApplication",
            create_request("CasedApp", &app_block_arn),
        )
        .await;
    emulator
        .ok(
            "setup",
            "CreateApplication",
            create_request("deleted-later", &app_block_arn),
        )
        .await;
    emulator
        .ok(
            "setup",
            "DeleteApplication",
            json!({ "Name": "deleted-later" }),
        )
        .await;

    let mut names = absent_names();
    names.push("casedapp".to_string());
    names.push("deleted-later".to_string());

    for name in names {
        let case = format!("UpdateApplication {name:?}");
        let message = refusal(
            &emulator,
            &case,
            "UpdateApplication",
            json!({ "Name": name, "Description": "an update nobody applies" }),
            404,
            "ResourceNotFoundException",
        )
        .await;
        let expected = not_found_sentence(APPLICATION_KIND, &name);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_app_block_arn_that_addresses_no_app_block_is_not_found() {
    let emulator = Emulator::new();
    let app_block_arn = create_app_block(&emulator, "setup", APP_BLOCK).await;
    let other_app_block_arn = create_app_block(&emulator, "setup", OTHER_APP_BLOCK).await;
    let created = emulator
        .ok(
            "setup",
            "CreateApplication",
            create_request("rebound", &app_block_arn),
        )
        .await;
    let before = without_icon_url(&created["Application"]);

    // Rows: ARNs that pass the modelled `Arn` pattern and address no app block this
    // account holds — a name nothing was created under, an app block of another account,
    // one of another Region, another resource type of this service, and another service's
    // resource altogether.
    for (case, arn) in [
        (
            "a name no app block was created under",
            format!("arn:aws:appstream:{REGION}:{ACCOUNT}:app-block/absent"),
        ),
        (
            "an app block of another account",
            format!("arn:aws:appstream:{REGION}:210987654321:app-block/{APP_BLOCK}"),
        ),
        (
            "an app block of another Region",
            format!("arn:aws:appstream:eu-west-1:{ACCOUNT}:app-block/{APP_BLOCK}"),
        ),
        (
            "another resource type of this service",
            format!("arn:aws:appstream:{REGION}:{ACCOUNT}:fleet/{APP_BLOCK}"),
        ),
        (
            "another service's resource",
            format!("arn:aws:s3:::app-packages/{APP_BLOCK}"),
        ),
    ] {
        let message = refusal(
            &emulator,
            case,
            "UpdateApplication",
            json!({ "Name": "rebound", "AppBlockArn": arn, "Description": "not applied" }),
            404,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message,
            not_found_sentence(APP_BLOCK_ARN_KIND, &arn),
            "case {case:?}: expected AWS's sentence for the app block the request named, \
             got {message:?}"
        );

        let described = describe_one(&emulator, case, "rebound").await;
        assert_eq!(
            without_icon_url(&described),
            before,
            "case {case:?}: a refused update must store nothing, got {described}"
        );
    }

    // The positive control: the same request against the app block that EXISTS succeeds.
    let updated = emulator
        .ok(
            "positive control",
            "UpdateApplication",
            json!({
                "Name": "rebound",
                "AppBlockArn": other_app_block_arn,
                "Description": "applied",
            }),
        )
        .await;
    assert_eq!(
        updated["Application"]["AppBlockArn"],
        json!(other_app_block_arn),
        "case \"positive control\": an update naming a stored app block must be applied, \
         got {updated}"
    );
}

#[tokio::test]
async fn setting_and_deleting_the_same_field_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let app_block_arn = create_app_block(&emulator, "setup", APP_BLOCK).await;
    let mut create = create_request("conflicting", &app_block_arn);
    for (_, member, value) in DELETABLE_ATTRIBUTES {
        create[*member] = json!(value);
    }
    let created = emulator.ok("setup", "CreateApplication", create).await;
    let before = without_icon_url(&created["Application"]);

    // One row per deletable attribute: the request both sets the member and deletes it,
    // which asks for two outcomes for one field.
    for (attribute, member, _) in DELETABLE_ATTRIBUTES {
        let case = format!("{member} set and deleted at once");
        let mut request =
            json!({ "Name": "conflicting", "AttributesToDelete": [attribute] });
        request[*member] = json!("a value nobody stores");
        assert_error(
            &emulator,
            &case,
            "UpdateApplication",
            request,
            400,
            "ValidationException",
        )
        .await;
    }

    // Not an `ApplicationAttribute` value, so there is no stored field it could name.
    assert_error(
        &emulator,
        "unknown application attribute",
        "UpdateApplication",
        json!({ "Name": "conflicting", "AttributesToDelete": ["NOT_AN_APPLICATION_ATTRIBUTE"] }),
        400,
        "ValidationException",
    )
    .await;

    let described = describe_one(&emulator, "after the refusals", "conflicting").await;
    assert_eq!(
        without_icon_url(&described),
        before,
        "a refused UpdateApplication must store nothing, got {described}"
    );
}
