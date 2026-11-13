// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's app block deletion over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * an app block a stored application still names as the source of its binaries is IN
//!   USE: the delete is refused with `ResourceInUseException` (400) carrying AWS's
//!   sentence for that class, and removes nothing — however many applications name it,
//!   and until every one of them is deleted;
//! * once nothing names it, the delete answers the empty result and the app block is gone
//!   for every later caller: a read of its ARN and a repeat delete of its name are both
//!   refused `ResourceNotFoundException` (400) with AWS's sentence, so "gone" means
//!   unusable rather than merely unlisted;
//! * a name this account holds no app block under is refused the same way, whether it was
//!   never created or has already been deleted.

use serde_json::{json, Value};

use crate::refusal_messages::refusal;
use crate::stack_fleet_lifecycle::Emulator;

/// The account every emulator ARN and every refusal sentence names.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region every case below signs for.
const REGION: &str = "us-east-1";

/// AWS's sentence for an app block in use, and for one this account does not hold, read
/// from the crate so the test tracks the wording the service states rather than a copy.
fn in_use_sentence(name: &str) -> String {
    service_com_amazonaws_appstream_photonadminproxyservice::state::app_block_in_use_message(name)
}

fn not_found_sentence(arn: &str) -> String {
    service_com_amazonaws_appstream_photonadminproxyservice::state::app_block_not_found_message(arn)
}

/// The ARN an app block of this name carries in [`REGION`].
fn app_block_arn(name: &str) -> String {
    format!("arn:aws:appstream:{REGION}:{ACCOUNT}:app-block/{name}")
}

/// One row of the deletion table: how the app block is packaged, what its name looks
/// like, and how many applications are created against it.
struct Row {
    case: &'static str,
    name: &'static str,
    /// The `PackagingType` the create names, if any — `CUSTOM` needs a setup script and
    /// `APPSTREAM2` admits a post-setup one
    /// (`smithy_query(query="operation", operation="CreateAppBlock")`).
    packaging_type: Option<&'static str>,
    /// How many applications name this app block's ARN, each of which blocks the delete.
    applications: usize,
}

/// The deletion table. The rows differ along what the rule generalises over: the form and
/// length of the app block name, the packaging the app block was created with, and the
/// NUMBER of applications holding it — including none, where the delete is admitted
/// straight away.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "appstream2 packaging, one application",
            name: "deletable-appstream2",
            packaging_type: Some("APPSTREAM2"),
            applications: 1,
        },
        Row {
            case: "custom packaging, one application",
            name: "custom.block_1-9",
            packaging_type: Some("CUSTOM"),
            applications: 1,
        },
        Row {
            case: "no packaging type named, three applications",
            name: "0block",
            packaging_type: None,
            applications: 3,
        },
        Row {
            case: "appstream2 packaging, nothing holds it",
            name: "unheld-block",
            packaging_type: Some("APPSTREAM2"),
            applications: 0,
        },
        Row {
            case: "a long name, two applications",
            // `Name`'s pattern is ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$, and the
            // applications created against this block are named after it, so the row
            // leaves room for their suffix.
            name: "z99999999999999999999999999999999999999999999999999999999999999999999999999999999999",
            packaging_type: Some("APPSTREAM2"),
            applications: 2,
        },
    ]
}

/// The `CreateAppBlock` request a row asks for.
fn create_app_block(row: &Row) -> Value {
    let mut request = json!({
        "Name": row.name,
        "SourceS3Location": { "S3Bucket": "app-packages", "S3Key": "disks/app.vhd" },
    });
    if let Some(packaging_type) = row.packaging_type {
        request["PackagingType"] = json!(packaging_type);
    }
    // "SetupScriptDetails must be provided for the CUSTOM PackagingType", which is also
    // the packaging a request naming none gets.
    if row.packaging_type.is_none_or(|packaging| packaging == "CUSTOM") {
        request["SetupScriptDetails"] = json!({
            "ScriptS3Location": { "S3Bucket": "app-packages", "S3Key": "scripts/setup.ps1" },
            "ExecutablePath": "scripts/setup.ps1",
            "TimeoutInSeconds": 60,
        });
    }
    request
}

/// The `CreateApplication` request for the `index`-th application of a row.
fn create_application(row: &Row, index: usize, app_block_arn: &str) -> Value {
    json!({
        "Name": format!("{}-app-{index}", row.name),
        "AppBlockArn": app_block_arn,
        "Platforms": ["WINDOWS_SERVER_2019"],
        "InstanceFamilies": ["GENERAL_PURPOSE"],
        "IconS3Location": { "S3Bucket": "app-packages", "S3Key": "icons/app.png" },
        "LaunchPath": "C:\\app\\app.exe",
    })
}

/// The app blocks a read of one ARN reports.
async fn described(emulator: &Emulator, case: &str, arn: &str) -> Vec<Value> {
    let payload = emulator
        .ok(case, "DescribeAppBlocks", json!({ "Arns": [arn] }))
        .await;
    payload
        .get("AppBlocks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn an_app_block_an_application_holds_may_not_be_deleted_until_it_is_released() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::in_region(REGION);
        let created = emulator.ok(case, "CreateAppBlock", create_app_block(&row)).await;
        let arn = created
            .get("AppBlock")
            .and_then(|block| block.get("Arn"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected the app block's ARN, got {created}"))
            .to_string();
        assert_eq!(
            arn,
            app_block_arn(row.name),
            "case {case:?}: the created app block must carry its own ARN"
        );

        let applications: Vec<String> = (0..row.applications)
            .map(|index| {
                let request = create_application(&row, index, &arn);
                request["Name"].as_str().unwrap_or_default().to_string()
            })
            .collect();
        for index in 0..row.applications {
            emulator
                .ok(case, "CreateApplication", create_application(&row, index, &arn))
                .await;
        }

        // Every application still holding the app block blocks the delete, and the
        // refusal leaves the app block exactly where it was.
        for (held, application) in applications.iter().enumerate() {
            let message = refusal(
                &emulator,
                case,
                "DeleteAppBlock",
                json!({ "Name": row.name }),
                400,
                "ResourceInUseException",
            )
            .await;
            assert_eq!(
                message,
                in_use_sentence(row.name),
                "case {case:?}: with {} application(s) left the refusal must state that the \
                 app block is in use",
                row.applications - held
            );
            assert_eq!(
                described(&emulator, case, &arn).await.len(),
                1,
                "case {case:?}: the refused delete must leave the app block stored"
            );
            emulator
                .ok(case, "DeleteApplication", json!({ "Name": application }))
                .await;
        }

        // Nothing holds it now, so the delete is admitted and answers the empty result.
        let deleted = emulator
            .ok(case, "DeleteAppBlock", json!({ "Name": row.name }))
            .await;
        assert_eq!(
            deleted,
            json!({}),
            "case {case:?}: the modelled result carries no members, got {deleted}"
        );

        // Gone means unusable: the ARN addresses no app block, and the name may not be
        // deleted again — both refused with AWS's sentence naming the ARN.
        for (operation, request) in [
            ("DescribeAppBlocks", json!({ "Arns": [arn.clone()] })),
            ("DeleteAppBlock", json!({ "Name": row.name })),
        ] {
            let message = refusal(
                &emulator,
                case,
                operation,
                request,
                400,
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                message,
                not_found_sentence(&arn),
                "case {case:?}: {operation} on the deleted app block must answer AWS's \
                 not-found sentence"
            );
        }

        // A read that names no ARN reports what is left, which is nothing.
        let remaining = emulator.ok(case, "DescribeAppBlocks", json!({})).await;
        assert_eq!(
            remaining
                .get("AppBlocks")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default(),
            0,
            "case {case:?}: the deleted app block must be gone from the catalog, got \
             {remaining}"
        );
    }
}

#[tokio::test]
async fn a_name_this_account_holds_no_app_block_under_is_not_found() {
    let emulator = Emulator::in_region(REGION);
    // Rows: names spanning the class the `Name` pattern admits, none of them created.
    for name in [
        "absent",
        "Mixed.Case_name-9",
        "0123456789",
        "z99999999999999999999999999999999999999999999999999",
    ] {
        let message = refusal(
            &emulator,
            name,
            "DeleteAppBlock",
            json!({ "Name": name }),
            400,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message,
            not_found_sentence(&app_block_arn(name)),
            "case {name:?}: the refusal must name the ARN the deleted name addresses"
        );
    }
}

#[tokio::test]
async fn deleting_an_application_releases_only_the_app_block_it_held() {
    // Two app blocks, one application each: releasing one must not make the other
    // deletable, so the guard reads the application's OWN `AppBlockArn` rather than
    // whether any application exists at all.
    let case = "two app blocks, one application each";
    let emulator = Emulator::in_region(REGION);
    let table = rows();
    let (first, second) = (&table[0], &table[1]);

    for row in [first, second] {
        emulator.ok(case, "CreateAppBlock", create_app_block(row)).await;
        emulator
            .ok(
                case,
                "CreateApplication",
                create_application(row, 0, &app_block_arn(row.name)),
            )
            .await;
    }

    emulator
        .ok(
            case,
            "DeleteApplication",
            json!({ "Name": format!("{}-app-0", first.name) }),
        )
        .await;

    emulator
        .ok(case, "DeleteAppBlock", json!({ "Name": first.name }))
        .await;
    let message = refusal(
        &emulator,
        case,
        "DeleteAppBlock",
        json!({ "Name": second.name }),
        400,
        "ResourceInUseException",
    )
    .await;
    assert_eq!(
        message,
        in_use_sentence(second.name),
        "case {case:?}: the app block whose application is still there stays in use"
    );
    assert_eq!(
        described(&emulator, case, &app_block_arn(second.name)).await.len(),
        1,
        "case {case:?}: the app block still held must still be stored"
    );
}
