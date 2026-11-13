// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the attribute map an application reports, through the whole
//! create/update/read lifecycle.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * `Application.Metadata` reports the additional attributes that describe the
//!   application: one entry per attribute the application HAS, keyed by the modelled
//!   `ApplicationAttribute` names (`APPLICATION_ATTRIBUTE_VALUES` in
//!   `src/ops/types/shared_types.rs`), so the key set follows what a create or an update
//!   supplied and an `AttributesToDelete` deletion drops its key;
//! * every member an `UpdateApplication` request carries replaces that field of the
//!   stored application and a member it omits leaves that field alone, so the following
//!   `DescribeApplications` serves the WHOLE updated application — the attribute map
//!   included — rather than the pre-update one.
//!
//! `tests/all/update_application.rs` covers that operation's own refusals (an unknown
//! application, an `AppBlockArn` that addresses nothing, an attribute both set and
//! deleted); this suite is about what the application reports afterwards.

use serde_json::{json, Value};

use crate::applications::{
    application_arn, catalog, create_app_block, describe, expected_metadata,
    LAUNCH_PARAMETERS_ATTRIBUTE, WORKING_DIRECTORY_ATTRIBUTE,
};
use crate::stack_fleet_lifecycle::Emulator;

/// The app block every application here is created against, and a second one an update
/// can re-point an application at.
const APP_BLOCK: &str = "catalog-block";
const OTHER_APP_BLOCK: &str = "other-block";

/// The placeholder an update row carries where it means "the second app block this test
/// creates": the real ARN is only known once the app block exists.
const OTHER_APP_BLOCK_PLACEHOLDER: &str = "<other-app-block>";

/// The platforms and instance families every application here is created with; neither is
/// an `UpdateApplication` member, so they are fixed across the table.
const PLATFORMS: &[&str] = &["WINDOWS_SERVER_2019"];
const INSTANCE_FAMILIES: &[&str] = &["GENERAL_PURPOSE"];

/// The bucket every application's icon lives in before an update moves it.
const ICON_BUCKET: &str = "app-icons";

/// What this test believes the stored application is, so the expected response is
/// computed from the rule (every supplied member replaces the stored one, every deleted
/// attribute is gone) rather than from the emulator's own answer.
#[derive(Clone, Debug)]
struct Model {
    name: String,
    display_name: Option<String>,
    description: Option<String>,
    launch_path: String,
    launch_parameters: Option<String>,
    working_directory: Option<String>,
    app_block_arn: String,
    icon_bucket: String,
    icon_key: String,
}

impl Model {
    /// The `CreateApplication` request that persists this application.
    fn create_request(&self) -> Value {
        let mut request = json!({
            "Name": self.name,
            "AppBlockArn": self.app_block_arn,
            "IconS3Location": { "S3Bucket": self.icon_bucket, "S3Key": self.icon_key },
            "LaunchPath": self.launch_path,
            "Platforms": PLATFORMS,
            "InstanceFamilies": INSTANCE_FAMILIES,
        });
        for (member, value) in [
            ("DisplayName", &self.display_name),
            ("Description", &self.description),
            ("LaunchParameters", &self.launch_parameters),
            ("WorkingDirectory", &self.working_directory),
        ] {
            if let Some(value) = value {
                request[member] = json!(value);
            }
        }
        request
    }

    /// Apply the update the way the rule says the service does: a member the request
    /// carries replaces the stored one, a member it omits changes nothing, and every
    /// attribute `AttributesToDelete` names ends up gone.
    fn apply(&mut self, update: &Value) {
        for (member, field) in [
            ("DisplayName", &mut self.display_name),
            ("Description", &mut self.description),
            ("LaunchParameters", &mut self.launch_parameters),
            ("WorkingDirectory", &mut self.working_directory),
        ] {
            if let Some(value) = update.get(member).and_then(Value::as_str) {
                *field = Some(value.to_string());
            }
        }
        if let Some(launch_path) = update.get("LaunchPath").and_then(Value::as_str) {
            self.launch_path = launch_path.to_string();
        }
        if let Some(app_block_arn) = update.get("AppBlockArn").and_then(Value::as_str) {
            self.app_block_arn = app_block_arn.to_string();
        }
        if let Some(location) = update.get("IconS3Location") {
            if let Some(bucket) = location.get("S3Bucket").and_then(Value::as_str) {
                self.icon_bucket = bucket.to_string();
            }
            if let Some(key) = location.get("S3Key").and_then(Value::as_str) {
                self.icon_key = key.to_string();
            }
        }
        if let Some(attributes) = update.get("AttributesToDelete").and_then(Value::as_array) {
            for attribute in attributes.iter().filter_map(Value::as_str) {
                if attribute == LAUNCH_PARAMETERS_ATTRIBUTE {
                    self.launch_parameters = None;
                } else if attribute == WORKING_DIRECTORY_ATTRIBUTE {
                    self.working_directory = None;
                }
            }
        }
    }

    /// The modelled `Application` this stored application must be reported as, given the
    /// `CreatedTime` its create stamped. `IconURL` is signed per render, so it is asserted
    /// on its own rather than compared here.
    fn expected(&self, created_time: &Value) -> Value {
        let mut expected = json!({
            "Arn": application_arn(&self.name),
            "Name": self.name,
            "AppBlockArn": self.app_block_arn,
            "IconS3Location": { "S3Bucket": self.icon_bucket, "S3Key": self.icon_key },
            "LaunchPath": self.launch_path,
            "Platforms": PLATFORMS,
            "InstanceFamilies": INSTANCE_FAMILIES,
            "Enabled": true,
            "CreatedTime": created_time.clone(),
            "Metadata": expected_metadata(
                self.launch_parameters.as_deref(),
                self.working_directory.as_deref(),
            ),
        });
        // A member the modelled shape declares `@length` 1.. is absent for an application
        // whose value for it is the empty string, as every other reader of this service
        // renders such a value.
        for (member, value) in [
            ("DisplayName", &self.display_name),
            ("Description", &self.description),
            ("LaunchParameters", &self.launch_parameters),
            ("WorkingDirectory", &self.working_directory),
        ] {
            if let Some(value) = value.as_deref().filter(|value| !value.is_empty()) {
                expected[member] = json!(value);
            }
        }
        expected
    }
}

/// One row of the update table: the application to create and the update to send it.
struct UpdateRow {
    case: &'static str,
    display_name: Option<&'static str>,
    description: Option<&'static str>,
    launch_path: &'static str,
    launch_parameters: Option<&'static str>,
    working_directory: Option<&'static str>,
    /// The members the update carries beside `Name`.
    update: Value,
}

/// Update requests spanning the input class: the rows differ along WHICH members the
/// request carries (one, several, all of them, none), along whether the application
/// already had a value for the member, along the form of the value (a Windows path, a
/// POSIX path, flags with spaces, the empty string a `@length` 0.. member allows), and
/// along how many attributes `AttributesToDelete` names (none, one, the modelled maximum
/// of two).
///
/// Member names come from
/// `smithy_query(query="operation", operation="UpdateApplication")`; the attribute values
/// are `ApplicationAttribute`'s (`APPLICATION_ATTRIBUTE_VALUES` in
/// `src/ops/types/shared_types.rs`).
fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "the display name and the launch path replaced",
            display_name: Some("Before"),
            description: Some("kept"),
            launch_path: "C:\\Program Files\\App\\before.exe",
            launch_parameters: None,
            working_directory: Some("C:\\Program Files\\App"),
            update: json!({
                "DisplayName": "After",
                "LaunchPath": "C:\\Program Files\\App\\after.exe",
            }),
        },
        UpdateRow {
            case: "every updatable member replaced at once",
            display_name: Some("Before"),
            description: Some("before"),
            launch_path: "/opt/app/bin/before",
            launch_parameters: Some("--before"),
            working_directory: Some("/opt/app"),
            update: json!({
                "DisplayName": "After",
                "Description": "after",
                "LaunchPath": "/opt/app/bin/after",
                "LaunchParameters": "--after --locale=en_GB",
                "WorkingDirectory": "/opt/app/after",
                "IconS3Location": { "S3Bucket": "other-icons", "S3Key": "after/icon.png" },
                "AppBlockArn": OTHER_APP_BLOCK_PLACEHOLDER,
            }),
        },
        UpdateRow {
            case: "a request that carries nothing but the name",
            display_name: Some("Untouched"),
            description: Some("untouched"),
            launch_path: "/usr/bin/app",
            launch_parameters: Some("-v"),
            working_directory: Some("/usr"),
            update: json!({}),
        },
        UpdateRow {
            case: "attributes set on an application created without them",
            display_name: None,
            description: None,
            launch_path: "C:\\app.exe",
            launch_parameters: None,
            working_directory: None,
            update: json!({
                "LaunchParameters": "--kiosk",
                "WorkingDirectory": "C:\\",
            }),
        },
        UpdateRow {
            case: "the working directory deleted",
            display_name: Some("Deleting"),
            description: None,
            launch_path: "/opt/app/bin/app",
            launch_parameters: Some("--kiosk"),
            working_directory: Some("/opt/app"),
            update: json!({ "AttributesToDelete": [WORKING_DIRECTORY_ATTRIBUTE] }),
        },
        UpdateRow {
            case: "both attributes deleted in one request",
            display_name: Some("Deleting"),
            description: Some("kept"),
            launch_path: "/opt/app/bin/app",
            launch_parameters: Some("--kiosk"),
            working_directory: Some("/opt/app"),
            update: json!({
                "AttributesToDelete": [LAUNCH_PARAMETERS_ATTRIBUTE, WORKING_DIRECTORY_ATTRIBUTE],
            }),
        },
        UpdateRow {
            case: "the display name cleared with the empty string",
            display_name: Some("Before"),
            description: Some("before"),
            launch_path: "/opt/app/bin/app",
            launch_parameters: None,
            working_directory: Some("/opt/app"),
            update: json!({ "DisplayName": "", "Description": "" }),
        },
        UpdateRow {
            case: "the launch path of an application re-pointed at another app block",
            display_name: None,
            description: None,
            launch_path: "C:\\old\\app.exe",
            launch_parameters: None,
            working_directory: Some("C:\\old"),
            update: json!({
                "AppBlockArn": OTHER_APP_BLOCK_PLACEHOLDER,
                "LaunchPath": "C:\\new\\app.exe",
                "WorkingDirectory": "C:\\new",
            }),
        },
    ]
}

impl UpdateRow {
    /// The application this row creates, under a name of its own.
    fn model(&self, index: usize, app_block_arn: &str) -> Model {
        Model {
            name: format!("updated-{index:02}"),
            display_name: self.display_name.map(str::to_string),
            description: self.description.map(str::to_string),
            launch_path: self.launch_path.to_string(),
            launch_parameters: self.launch_parameters.map(str::to_string),
            working_directory: self.working_directory.map(str::to_string),
            app_block_arn: app_block_arn.to_string(),
            icon_bucket: ICON_BUCKET.to_string(),
            icon_key: format!("icons/{index:02}.png"),
        }
    }

    /// The update request this row sends, addressed to `name` and with the placeholder
    /// resolved to the ARN of the second app block.
    fn request(&self, name: &str, other_app_block_arn: &str) -> Value {
        let mut update = self.update.clone();
        update["Name"] = json!(name);
        if update.get("AppBlockArn").and_then(Value::as_str) == Some(OTHER_APP_BLOCK_PLACEHOLDER) {
            update["AppBlockArn"] = json!(other_app_block_arn);
        }
        update
    }
}

/// The `Application` of a response that carries one.
fn application(case: &str, operation: &str, payload: &Value) -> Value {
    payload
        .get("Application")
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an Application from {operation}, got {payload}")
        })
}

/// The application without the `IconURL` it reports: that URL is signed per render
/// against the clock of the call that reports it, so it is asserted on its own.
fn without_icon_url(application: &Value) -> Value {
    let mut application = application.clone();
    if let Some(object) = application.as_object_mut() {
        object.remove("IconURL");
    }
    application
}

// ---------------------------------------------------------------------------
// The update, and the read that follows it
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_supplied_member_is_visible_to_the_following_describe() {
    for (index, row) in update_rows().into_iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::new();
        let app_block = create_app_block(&emulator, case, APP_BLOCK).await;
        let other_app_block = create_app_block(&emulator, case, OTHER_APP_BLOCK).await;

        let mut model = row.model(index, &app_block);
        let created = emulator
            .ok(case, "CreateApplication", model.create_request())
            .await;
        let created = application(case, "CreateApplication", &created);
        let created_time = created
            .get("CreatedTime")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreatedTime, got {created}"));
        let before = model.expected(&created_time);
        assert_eq!(
            without_icon_url(&created),
            before,
            "case {case:?}: the create must report the record it persisted, got {created}"
        );

        let request = row.request(&model.name, &other_app_block);
        let updated = emulator.ok(case, "UpdateApplication", request.clone()).await;
        let updated = application(case, "UpdateApplication", &updated);

        model.apply(&request);
        let expected = model.expected(&created_time);
        assert_eq!(
            without_icon_url(&updated),
            expected,
            "case {case:?}: the update must report the record it wrote, got {updated}"
        );

        let (applications, _) = describe(
            &emulator,
            case,
            json!({ "Arns": [application_arn(&model.name)] }),
        )
        .await;
        assert_eq!(
            applications.len(),
            1,
            "case {case:?}: the updated application must still be addressed by its ARN, got \
             {applications:?}"
        );
        assert_eq!(
            without_icon_url(&applications[0]),
            expected,
            "case {case:?}: the read after the update must serve the updated application, got {:?}",
            applications[0]
        );

        // The icon URL is still a signed address of the icon the application now names.
        let icon_url = applications[0]
            .get("IconURL")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an IconURL, got {:?}", applications[0])
            });
        assert!(
            icon_url.contains(&model.icon_bucket) && icon_url.contains("X-Amz-Signature="),
            "case {case:?}: the icon URL must be a signed address of the stored icon, got \
             {icon_url:?}"
        );

        // Reading again reports the same application: a read changes nothing, and the
        // update is still the latest write.
        let again = catalog(&emulator, case).await;
        assert_eq!(
            again.len(),
            1,
            "case {case:?}: exactly one application must be stored, got {again:?}"
        );
        assert_eq!(
            without_icon_url(&again[0]),
            expected,
            "case {case:?}: a read must not change what a later caller sees, got {:?}",
            again[0]
        );
    }
}

#[tokio::test]
async fn updates_accumulate_on_the_stored_application() {
    // Rows: one update per step, each replacing a different member, so the application
    // the last read serves carries the value of EVERY step rather than only the last.
    let emulator = Emulator::new();
    let app_block = create_app_block(&emulator, "setup", APP_BLOCK).await;
    let mut model = Model {
        name: "accumulating".to_string(),
        display_name: None,
        description: None,
        launch_path: "C:\\app.exe".to_string(),
        launch_parameters: None,
        working_directory: None,
        app_block_arn: app_block.clone(),
        icon_bucket: ICON_BUCKET.to_string(),
        icon_key: "icons/accumulating.png".to_string(),
    };
    let created = emulator
        .ok("setup", "CreateApplication", model.create_request())
        .await;
    let created_time = application("setup", "CreateApplication", &created)
        .get("CreatedTime")
        .cloned()
        .unwrap_or_else(|| panic!("case \"setup\": expected a CreatedTime, got {created}"));

    for (case, update) in [
        ("the display name", json!({ "DisplayName": "Accumulating" })),
        ("the working directory", json!({ "WorkingDirectory": "C:\\" })),
        ("the launch parameters", json!({ "LaunchParameters": "--kiosk" })),
        ("the launch path", json!({ "LaunchPath": "C:\\bin\\app.exe" })),
        ("the description", json!({ "Description": "accumulated" })),
        (
            "the working directory again",
            json!({ "WorkingDirectory": "C:\\bin" }),
        ),
        (
            "the launch parameters deleted",
            json!({ "AttributesToDelete": [LAUNCH_PARAMETERS_ATTRIBUTE] }),
        ),
    ] {
        let mut request = update.clone();
        request["Name"] = json!(model.name);
        let updated = emulator.ok(case, "UpdateApplication", request.clone()).await;
        model.apply(&request);
        let expected = model.expected(&created_time);
        assert_eq!(
            without_icon_url(&application(case, "UpdateApplication", &updated)),
            expected,
            "case {case:?}: the update must report every earlier write too, got {updated}"
        );

        let read = catalog(&emulator, case).await;
        assert_eq!(
            without_icon_url(&read[0]),
            expected,
            "case {case:?}: the read must serve every write so far, got {:?}",
            read[0]
        );
    }
}

// ---------------------------------------------------------------------------
// The attribute map
// ---------------------------------------------------------------------------

#[tokio::test]
async fn metadata_is_keyed_by_the_attributes_the_application_has() {
    // Rows: the attribute values an application has — neither attribute, each one alone,
    // both, and values whose form differs (a Windows path, a POSIX path, a drive root,
    // flags with spaces).
    let emulator = Emulator::new();
    let app_block = create_app_block(&emulator, "setup", APP_BLOCK).await;
    for (index, (launch_parameters, working_directory)) in [
        (None, None),
        (None, Some("C:\\Program Files\\App")),
        (Some("--kiosk"), None),
        (Some("--kiosk --locale=en_GB"), Some("/opt/app")),
        (Some("-v"), Some("C:\\")),
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!(
            "LaunchParameters {launch_parameters:?}, WorkingDirectory {working_directory:?}"
        );
        let model = Model {
            name: format!("metadata-{index:02}"),
            display_name: None,
            description: None,
            launch_path: "C:\\app.exe".to_string(),
            launch_parameters: launch_parameters.map(str::to_string),
            working_directory: working_directory.map(str::to_string),
            app_block_arn: app_block.clone(),
            icon_bucket: ICON_BUCKET.to_string(),
            icon_key: format!("icons/metadata-{index:02}.png"),
        };
        emulator
            .ok(&case, "CreateApplication", model.create_request())
            .await;
        let (applications, _) = describe(
            &emulator,
            &case,
            json!({ "Arns": [application_arn(&model.name)] }),
        )
        .await;
        let metadata = applications[0]
            .get("Metadata")
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a Metadata map, got {:?}",
                    applications[0]
                )
            });
        assert_eq!(
            metadata,
            expected_metadata(launch_parameters, working_directory),
            "case {case:?}: Metadata must carry one entry per attribute the application \
             has, got {metadata}"
        );

        // The map and the members that carry the same attributes agree, entry by entry.
        for (attribute, member) in [
            (LAUNCH_PARAMETERS_ATTRIBUTE, "LaunchParameters"),
            (WORKING_DIRECTORY_ATTRIBUTE, "WorkingDirectory"),
        ] {
            assert_eq!(
                metadata.get(attribute),
                applications[0].get(member),
                "case {case:?}: the {attribute} entry must carry the application's {member}, \
                 got {metadata}"
            );
        }
    }
}
