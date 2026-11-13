// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's application catalog over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * an application is created against an app block that EXISTS: an `AppBlockArn` that
//!   addresses no app block this account holds is refused with
//!   `ResourceNotFoundException` (404) carrying AWS's sentence for that class, and no
//!   application is created for it — whatever else the request is well-formed about;
//! * a create that passes that gate persists the application and reports it back from the
//!   stored record, so a later `DescribeApplications` answers with the same ARN, launch
//!   details and creation instant;
//! * the name is allocated once: a second create of a name that exists is refused with
//!   `ResourceAlreadyExistsException` (400) and changes nothing;
//! * a read reports only stored applications, in a stable order, answers about exactly
//!   the `Arns` it is asked about, and `MaxResults` sizes its page: following `NextToken`
//!   walks every stored application exactly once;
//! * the icon URL an application reports is a time-limited address of the S3 object the
//!   create named, signed so that it leads back to this emulator, and signed afresh by
//!   every call that renders the record — so what two calls report is the same record in
//!   every member but that URL, whatever the clock did between them;
//! * an application reports the additional attributes that describe it in `Metadata`,
//!   keyed by the modelled `ApplicationAttribute` names — asserted over the whole
//!   attribute class in `tests/all/application_update.rs`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use smithy_server_runtime::UNSERVED_URL_BASE;

use crate::refusal_messages::{already_exists_sentence, not_found_sentence, refusal};
use crate::stack_fleet_lifecycle::{assert_error, Emulator};
use crate::streaming_url::query_field;

/// The account every emulated resource belongs to.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the harness signs for, which is the region every ARN below carries.
const REGION: &str = "us-east-1";

/// The app block every row of the creation table is created against.
const APP_BLOCK: &str = "catalog-block";

/// The S3 bucket every row stores its icon in.
const ICON_BUCKET: &str = "app-icons";

/// How long a case that waits for a condition waits for it before giving up, and how long
/// it waits between looks. Generous: what is under test is the condition, never how fast
/// the host reaches it.
const DEADLINE: Duration = Duration::from_secs(60);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The resource word AWS's `CreateApplication` refusal uses for the app block an
/// application is created against, read from the crate so the test tracks the constant
/// rather than a copy of its value.
const APP_BLOCK_ARN_KIND: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::APP_BLOCK_ARN_KIND;

/// The resource word AWS's refusals use for an application.
const APPLICATION_KIND: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::APPLICATION_KIND;

/// How long the icon URL stays valid, as the crate states it.
const ICON_URL_VALIDITY_SECONDS: u64 =
    service_com_amazonaws_appstream_photonadminproxyservice::state::ICON_URL_VALIDITY.as_secs();

/// The `ApplicationAttribute` names an application's `Metadata` map is keyed by, read
/// from the crate so the tests track the modelled values rather than a copy of them.
pub(crate) const LAUNCH_PARAMETERS_ATTRIBUTE: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::APPLICATION_ATTRIBUTE_LAUNCH_PARAMETERS;
pub(crate) const WORKING_DIRECTORY_ATTRIBUTE: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::APPLICATION_ATTRIBUTE_WORKING_DIRECTORY;

/// One row of the creation table: the members the request carries beside the app block.
struct Row {
    case: &'static str,
    name: &'static str,
    display_name: Option<&'static str>,
    description: Option<&'static str>,
    launch_path: &'static str,
    launch_parameters: Option<&'static str>,
    working_directory: Option<&'static str>,
    /// Values of `PlatformType` (`PLATFORM_TYPE_VALUES` in `src/ops/types/shared_types.rs`).
    platforms: &'static [&'static str],
    /// `InstanceFamilies` is documented "Valid values are GENERAL_PURPOSE and GRAPHICS_G4".
    instance_families: &'static [&'static str],
    icon_key: &'static str,
}

/// The creation table. The rows differ along what the rule generalises over: which
/// optional members the request carries, how many platforms and instance families it
/// names, and the form of the launch path and the icon key.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the required members only",
            name: "minimal",
            display_name: None,
            description: None,
            launch_path: "C:\\Program Files\\App\\app.exe",
            launch_parameters: None,
            working_directory: None,
            platforms: &["WINDOWS_SERVER_2019"],
            instance_families: &["GENERAL_PURPOSE"],
            icon_key: "icons/minimal.png",
        },
        Row {
            case: "every optional member, several platforms",
            name: "fully.described-01",
            display_name: Some("Fully Described"),
            description: Some("an application with every optional member"),
            launch_path: "/opt/app/bin/app",
            launch_parameters: Some("--kiosk --locale=en_GB"),
            working_directory: Some("/opt/app"),
            platforms: &["AMAZON_LINUX2", "WINDOWS_SERVER_2019"],
            instance_families: &["GENERAL_PURPOSE", "GRAPHICS_G4"],
            icon_key: "icons/nested/path with spaces/app.png",
        },
        Row {
            case: "a graphics-only application",
            name: "graphics_only",
            display_name: Some("Graphics"),
            description: None,
            launch_path: "C:\\app.exe",
            launch_parameters: None,
            working_directory: Some("C:\\"),
            platforms: &["WINDOWS_SERVER_2019"],
            instance_families: &["GRAPHICS_G4"],
            icon_key: "icon.svg",
        },
        Row {
            case: "a display name but no description",
            name: "named",
            display_name: Some("Named"),
            description: None,
            launch_path: "/usr/bin/app",
            launch_parameters: Some("-v"),
            working_directory: None,
            platforms: &["UBUNTU_PRO_2404"],
            instance_families: &["GENERAL_PURPOSE"],
            icon_key: "a.png",
        },
    ]
}

/// The `arn:aws:appstream:…:app-block/<name>` of an app block of this account.
pub(crate) fn app_block_arn(name: &str) -> String {
    format!("arn:aws:appstream:{REGION}:{ACCOUNT}:app-block/{name}")
}

/// The `arn:aws:appstream:…:application/<name>` of an application of this account.
pub(crate) fn application_arn(name: &str) -> String {
    format!("arn:aws:appstream:{REGION}:{ACCOUNT}:application/{name}")
}

/// Create the app block every application below is created against.
///
/// Shared with the other suites in this target (`tests/all/update_application.rs`,
/// `tests/all/application_update.rs`), so every application suite stands its app block up
/// the same way.
pub(crate) async fn create_app_block(emulator: &Emulator, case: &str, name: &str) -> String {
    let created = emulator
        .ok(
            case,
            "CreateAppBlock",
            json!({
                "Name": name,
                "SourceS3Location": { "S3Bucket": "app-packages", "S3Key": "block.vhd" },
                "SetupScriptDetails": {
                    "ScriptS3Location": { "S3Bucket": "app-packages", "S3Key": "setup.ps1" },
                    "ExecutablePath": "setup.ps1",
                    "TimeoutInSeconds": 60,
                },
            }),
        )
        .await;
    created
        .pointer("/AppBlock/Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected the app block's ARN, got {created}"))
        .to_string()
}

/// The `CreateApplication` request a row asks for, against `app_block_arn`.
fn create_request(row: &Row, app_block_arn: &str) -> Value {
    let mut request = json!({
        "Name": row.name,
        "AppBlockArn": app_block_arn,
        "IconS3Location": { "S3Bucket": ICON_BUCKET, "S3Key": row.icon_key },
        "LaunchPath": row.launch_path,
        "Platforms": row.platforms,
        "InstanceFamilies": row.instance_families,
    });
    if let Some(display_name) = row.display_name {
        request["DisplayName"] = json!(display_name);
    }
    if let Some(description) = row.description {
        request["Description"] = json!(description);
    }
    if let Some(launch_parameters) = row.launch_parameters {
        request["LaunchParameters"] = json!(launch_parameters);
    }
    if let Some(working_directory) = row.working_directory {
        request["WorkingDirectory"] = json!(working_directory);
    }
    request
}

/// The application a row must be reported as, given the `CreatedTime` the create stamped.
///
/// `IconURL` is not a member of it: that URL is signed per render, so it is asserted with
/// [`assert_icon_url`] rather than compared, and the records compared here are
/// [`without_icon_url`].
fn expected_application(row: &Row, app_block_arn: &str, created: &Value) -> Value {
    let mut expected = json!({
        "Arn": application_arn(row.name),
        "Name": row.name,
        "AppBlockArn": app_block_arn,
        "IconS3Location": { "S3Bucket": ICON_BUCKET, "S3Key": row.icon_key },
        "LaunchPath": row.launch_path,
        "Platforms": row.platforms,
        "InstanceFamilies": row.instance_families,
        "Enabled": true,
        "CreatedTime": created.get("CreatedTime").cloned().unwrap_or(Value::Null),
    });
    if let Some(display_name) = row.display_name {
        expected["DisplayName"] = json!(display_name);
    }
    if let Some(description) = row.description {
        expected["Description"] = json!(description);
    }
    if let Some(launch_parameters) = row.launch_parameters {
        expected["LaunchParameters"] = json!(launch_parameters);
    }
    if let Some(working_directory) = row.working_directory {
        expected["WorkingDirectory"] = json!(working_directory);
    }
    // "Additional attributes that describe the application": the attributes the
    // application HAS, keyed by the modelled `ApplicationAttribute` names.
    expected["Metadata"] = expected_metadata(row.launch_parameters, row.working_directory);
    expected
}

/// The `Metadata` map an application with these attributes reports: one entry per
/// attribute it has, keyed by the modelled `ApplicationAttribute` name.
pub(crate) fn expected_metadata(
    launch_parameters: Option<&str>,
    working_directory: Option<&str>,
) -> Value {
    let mut metadata = serde_json::Map::new();
    if let Some(launch_parameters) = launch_parameters.filter(|value| !value.is_empty()) {
        metadata.insert(
            LAUNCH_PARAMETERS_ATTRIBUTE.to_string(),
            json!(launch_parameters),
        );
    }
    if let Some(working_directory) = working_directory.filter(|value| !value.is_empty()) {
        metadata.insert(
            WORKING_DIRECTORY_ATTRIBUTE.to_string(),
            json!(working_directory),
        );
    }
    Value::Object(metadata)
}

// ---------------------------------------------------------------------------
// The icon URL, which is signed per render
// ---------------------------------------------------------------------------

/// The application a call reported, without the icon URL it carries.
///
/// `Application.IconURL` is a time-limited address signed against the clock of the call
/// that renders the record (`ApplicationRecord::to_application` in `src/state.rs`), so two
/// calls that report the same stored application hand out URLs whose signing date and
/// signature differ as soon as a second elapses between them, while both address the same
/// S3 object. What two calls reported is therefore compared without that member, and the
/// member itself asserted with [`assert_icon_url`], which holds of every render whatever
/// the clock says.
pub(crate) fn without_icon_url(application: &Value) -> Value {
    let mut application = application.clone();
    if let Some(object) = application.as_object_mut() {
        object.remove("IconURL");
    }
    application
}

/// Every application a call reported, each [`without_icon_url`].
fn without_icon_urls(applications: &[Value]) -> Vec<Value> {
    applications.iter().map(without_icon_url).collect()
}

/// Assert the icon URL `application` reports is a signed, time-limited address of the S3
/// object it names: the rule that holds of EVERY render of the record.
pub(crate) fn assert_icon_url(case: &str, application: &Value, bucket: &str, key: &str) {
    let icon_url = icon_url(case, application);
    let (address, _) = icon_url
        .split_once('?')
        .unwrap_or_else(|| panic!("case {case:?}: expected a signed IconURL, got {icon_url:?}"));

    // The URL leads back to THIS emulator: the base the harness's router signs for, and
    // the path of the S3 object the application names.
    assert_eq!(
        percent_decoded(address),
        format!("{UNSERVED_URL_BASE}/{bucket}/{key}"),
        "case {case:?}: the icon URL must address the stored icon of this emulator, got \
         {icon_url:?}"
    );
    for field in ["X-Amz-Credential", "X-Amz-Date", "X-Amz-Signature"] {
        assert!(
            query_field(&icon_url, field).is_some_and(|value| !value.is_empty()),
            "case {case:?}: the icon URL must carry a {field} so it leads back to this \
             emulator, got {icon_url:?}"
        );
    }
    assert_eq!(
        query_field(&icon_url, "X-Amz-Expires"),
        Some(ICON_URL_VALIDITY_SECONDS.to_string()),
        "case {case:?}: the icon URL must be time-limited to the span the crate states, got \
         {icon_url:?}"
    );
}

/// The icon URL an application reports, which every render of a stored application
/// carries.
fn icon_url(case: &str, application: &Value) -> String {
    application
        .get("IconURL")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an IconURL, got {application}"))
        .to_string()
}

/// The instant each reported application's icon URL was signed at, as its `X-Amz-Date`
/// carries it: the per-render part of the record, and what tells one render's second from
/// another's.
fn signing_dates(case: &str, applications: &[Value]) -> Vec<String> {
    applications
        .iter()
        .map(|application| {
            let icon_url = icon_url(case, application);
            query_field(&icon_url, "X-Amz-Date").unwrap_or_else(|| {
                panic!("case {case:?}: expected a signed IconURL, got {icon_url:?}")
            })
        })
        .collect()
}

/// `text` with its `%XX` escapes decoded, so a signed path can be compared with the bucket
/// and key the application was created with.
fn percent_decoded(text: &str) -> String {
    percent_encoding::percent_decode_str(text)
        .decode_utf8_lossy()
        .into_owned()
}

/// The applications a `DescribeApplications` reports, and the token it carries.
pub(crate) async fn describe(
    emulator: &Emulator,
    case: &str,
    request: Value,
) -> (Vec<Value>, Option<Value>) {
    let payload = emulator.ok(case, "DescribeApplications", request).await;
    let applications = payload
        .get("Applications")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an Applications list, got {payload}"))
        .clone();
    (applications, payload.get("NextToken").cloned())
}

/// Every application of the account, in the order the read reports them.
pub(crate) async fn catalog(emulator: &Emulator, case: &str) -> Vec<Value> {
    let (applications, next_token) = describe(emulator, case, json!({})).await;
    assert!(
        next_token.is_none(),
        "case {case:?}: the whole catalog fits in one page here, got {next_token:?}"
    );
    applications
}

// ---------------------------------------------------------------------------
// Creating an application, and reading it back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_application_is_reported_from_the_stored_record() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let app_block_arn = create_app_block(&emulator, case, APP_BLOCK).await;

        let before = catalog(&emulator, case).await;
        assert!(
            before.is_empty(),
            "case {case:?}: an account with no applications must report none, got {before:?}"
        );

        let created = emulator
            .ok(case, "CreateApplication", create_request(&row, &app_block_arn))
            .await;
        let created = created
            .get("Application")
            .unwrap_or_else(|| panic!("case {case:?}: expected an Application, got {created}"))
            .clone();
        assert_eq!(
            without_icon_url(&created),
            expected_application(&row, &app_block_arn, &created),
            "case {case:?}: the create must report the record it persisted, got {created}"
        );
        assert_icon_url(case, &created, ICON_BUCKET, row.icon_key);

        let applications = catalog(&emulator, case).await;
        assert_eq!(
            applications.len(),
            1,
            "case {case:?}: exactly the created application must be reported, got \
             {applications:?}"
        );
        assert_eq!(
            without_icon_url(&applications[0]),
            without_icon_url(&created),
            "case {case:?}: the read must render the same record the create returned, got {:?}",
            applications[0]
        );
        assert_icon_url(case, &applications[0], ICON_BUCKET, row.icon_key);

        // Reading again reports the same application: the first read changed nothing, and
        // the creation instant belongs to the record rather than to the call that read it.
        let again = catalog(&emulator, case).await;
        assert_eq!(
            without_icon_urls(&again),
            without_icon_urls(&applications),
            "case {case:?}: a read must not change what a later caller sees, got {again:?}"
        );
        assert_icon_url(case, &again[0], ICON_BUCKET, row.icon_key);

        // The ARN the create minted is what the read is addressed by.
        let (addressed, _) = describe(
            &emulator,
            case,
            json!({ "Arns": [application_arn(row.name)] }),
        )
        .await;
        assert_eq!(
            without_icon_urls(&addressed),
            without_icon_urls(&applications),
            "case {case:?}: the ARN the create reported must address that application, got \
             {addressed:?}"
        );
        assert_icon_url(case, &addressed[0], ICON_BUCKET, row.icon_key);
    }
}

#[tokio::test]
async fn the_icon_url_is_a_time_limited_address_of_the_stored_icon() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let app_block_arn = create_app_block(&emulator, case, APP_BLOCK).await;
        let created = emulator
            .ok(case, "CreateApplication", create_request(&row, &app_block_arn))
            .await;
        let application = created
            .pointer("/Application")
            .unwrap_or_else(|| panic!("case {case:?}: expected an Application, got {created}"))
            .clone();

        // The URL addresses the bucket and key the create named, is signed so that it
        // leads back to this emulator, and is time-limited.
        assert_icon_url(case, &application, ICON_BUCKET, row.icon_key);
    }
}

#[tokio::test]
async fn a_read_in_another_second_reports_the_same_stored_applications() {
    let emulator = Emulator::new();
    let app_block_arn = create_app_block(&emulator, "setup", APP_BLOCK).await;
    for row in rows() {
        emulator
            .ok(row.case, "CreateApplication", create_request(&row, &app_block_arn))
            .await;
    }
    let first = catalog(&emulator, "the first read").await;
    assert_eq!(
        first.len(),
        rows().len(),
        "case \"the first read\": every created application must be reported, got {first:?}"
    );
    for application in &first {
        let name = application
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let row = rows()
            .into_iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| {
                panic!("the read must report a created application, got {name:?}")
            });
        assert_icon_url(row.case, application, ICON_BUCKET, row.icon_key);
    }

    // The icon URL is signed against the clock of the call that renders it, so its signing
    // date moves on with the second hand. Look again until a read falls in another second
    // — the condition this case needs — rather than waiting for a fixed time.
    let case = "a read in another second";
    let signed_first = signing_dates(case, &first);
    let deadline = Instant::now() + DEADLINE;
    let later = loop {
        let later = catalog(&emulator, case).await;
        let signed_later = signing_dates(case, &later);
        if signed_later != signed_first {
            break later;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the icon URL must be signed against the clock of the call that \
             renders it, still signed at {signed_later:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    };

    // Only that URL moved: the stored record a reader reports is the same in every other
    // member, so nothing a caller sees depends on which second the call fell in.
    assert_eq!(
        without_icon_urls(&later),
        without_icon_urls(&first),
        "case {case:?}: a later read must report the same stored applications, got {later:?}"
    );
    for row in rows() {
        let application = later
            .iter()
            .find(|application| application.get("Name") == Some(&json!(row.name)))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the later read must still report this application, got {later:?}",
                    row.case
                )
            });
        assert_icon_url(row.case, application, ICON_BUCKET, row.icon_key);
    }
}

// ---------------------------------------------------------------------------
// The app-block gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_app_block_arn_that_addresses_no_app_block_is_not_found() {
    // Rows: ARNs that pass the modelled `Arn` pattern and address no app block this
    // account holds — a name nothing was created under, an app block of another account,
    // one of another Region, another resource type of this service, and another service's
    // resource altogether.
    let emulator = Emulator::new();
    let stored = create_app_block(&emulator, "setup", APP_BLOCK).await;
    for (case, arn) in [
        ("a name no app block was created under", app_block_arn("absent")),
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
        let row = Row {
            case,
            name: "gated",
            display_name: None,
            description: None,
            launch_path: "C:\\app.exe",
            launch_parameters: None,
            working_directory: None,
            platforms: &["WINDOWS_SERVER_2019"],
            instance_families: &["GENERAL_PURPOSE"],
            icon_key: "icon.png",
        };
        let message = refusal(
            &emulator,
            case,
            "CreateApplication",
            create_request(&row, &arn),
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

        // The refused create persisted nothing: no application exists, and the name is
        // still free for a create against the app block that does exist.
        let applications = catalog(&emulator, case).await;
        assert!(
            applications.is_empty(),
            "case {case:?}: a refused create must persist nothing, got {applications:?}"
        );
    }

    // The positive control: the same request against the app block that EXISTS succeeds.
    let row = &rows()[0];
    emulator
        .ok("positive control", "CreateApplication", create_request(row, &stored))
        .await;
    assert_eq!(
        catalog(&emulator, "positive control").await.len(),
        1,
        "case \"positive control\": a create against a stored app block must persist it"
    );
}

#[tokio::test]
async fn a_second_create_of_a_name_that_exists_already_exists() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let app_block_arn = create_app_block(&emulator, case, APP_BLOCK).await;
        emulator
            .ok(case, "CreateApplication", create_request(&row, &app_block_arn))
            .await;
        let first = catalog(&emulator, case).await;

        let message = refusal(
            &emulator,
            case,
            "CreateApplication",
            create_request(&row, &app_block_arn),
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
        assert_eq!(
            message,
            already_exists_sentence(APPLICATION_KIND, row.name),
            "case {case:?}: expected AWS's duplicate-name sentence, got {message:?}"
        );

        let after = catalog(&emulator, case).await;
        assert_eq!(
            without_icon_urls(&after),
            without_icon_urls(&first),
            "case {case:?}: the refused create must leave the stored application exactly \
             as it was, got {after:?}"
        );
        assert_icon_url(case, &after[0], ICON_BUCKET, row.icon_key);
    }
}

// ---------------------------------------------------------------------------
// Reading the catalog
// ---------------------------------------------------------------------------

/// The application names the read cases create: enough records that every page size below
/// cuts the collection somewhere.
fn paged_names() -> Vec<String> {
    (1..=7).map(|index| format!("app-{index:02}")).collect()
}

#[tokio::test]
async fn following_next_token_reports_every_application_exactly_once() {
    let emulator = Emulator::new();
    let app_block_arn = create_app_block(&emulator, "setup", APP_BLOCK).await;
    for name in paged_names() {
        emulator
            .ok(
                "setup",
                "CreateApplication",
                json!({
                    "Name": name,
                    "AppBlockArn": app_block_arn,
                    "IconS3Location": { "S3Bucket": ICON_BUCKET, "S3Key": "icon.png" },
                    "LaunchPath": "C:\\app.exe",
                    "Platforms": ["WINDOWS_SERVER_2019"],
                    "InstanceFamilies": ["GENERAL_PURPOSE"],
                }),
            )
            .await;
    }
    let mut expected = paged_names();
    expected.sort();

    // Rows: the page size the request asks for — sizes that cut the collection, one that
    // holds it exactly, one larger than it, and a request that names no size at all.
    for max_results in [Some(1_i64), Some(2), Some(3), Some(7), Some(50), None] {
        let case = format!("MaxResults {max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<Value> = None;
        loop {
            let mut request = json!({});
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = token.clone();
            }
            let (applications, token) = describe(&emulator, &case, request).await;
            if let Some(max_results) = max_results {
                assert!(
                    i64::try_from(applications.len()).expect("a page fits in an i64") <= max_results,
                    "case {case:?}: a page must hold at most {max_results} applications, got {}",
                    applications.len()
                );
            }
            seen.extend(applications.iter().map(|application| {
                application
                    .get("Name")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: expected a Name, got {application}"))
                    .to_string()
            }));
            match token {
                Some(token) => next_token = Some(token),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: following NextToken must report every application exactly once, \
             in a stable order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_read_the_catalog_cannot_answer_is_refused() {
    let emulator = Emulator::new();
    let app_block_arn = create_app_block(&emulator, "setup", APP_BLOCK).await;
    emulator
        .ok(
            "setup",
            "CreateApplication",
            create_request(&rows()[0], &app_block_arn),
        )
        .await;

    // Rows: ARNs that address no stored application. Each is refused with AWS's sentence
    // for the resource the ARN names rather than answered with a short list.
    for (case, arn) in [
        ("a name nothing was created under", application_arn("absent")),
        (
            "an application of another account",
            format!("arn:aws:appstream:{REGION}:210987654321:application/minimal"),
        ),
        (
            "an application of another Region",
            format!("arn:aws:appstream:eu-west-1:{ACCOUNT}:application/minimal"),
        ),
    ] {
        let message = refusal(
            &emulator,
            case,
            "DescribeApplications",
            json!({ "Arns": [arn.clone()] }),
            404,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message,
            not_found_sentence(APPLICATION_KIND, &arn[arn.rfind('/').expect("an ARN with a resource id") + 1..]),
            "case {case:?}: expected AWS's sentence for the application the ARN names, got \
             {message:?}"
        );
    }

    // Rows: page sizes that cannot hold a record, and tokens this service never issued.
    for max_results in [0_i64, -1] {
        let case = format!("MaxResults {max_results}");
        assert_error(
            &emulator,
            &case,
            "DescribeApplications",
            json!({ "MaxResults": max_results }),
            400,
            "ValidationException",
        )
        .await;
    }
    for token in ["minimal", "abc", "zzzz"] {
        let case = format!("NextToken {token:?}");
        assert_error(
            &emulator,
            &case,
            "DescribeApplications",
            json!({ "NextToken": token }),
            400,
            "ValidationException",
        )
        .await;
    }

    // Every refusal left the stored application readable.
    assert_eq!(
        catalog(&emulator, "after refusals").await.len(),
        1,
        "case \"after refusals\": the stored application must still be reported"
    );
}
