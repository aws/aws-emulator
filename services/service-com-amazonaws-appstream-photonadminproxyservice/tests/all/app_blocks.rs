// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's app block lifecycle over the wire.
//!
//! Every case drives the real awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * `CreateAppBlock` persists the app block and reports it back from the stored record,
//!   so a later `DescribeAppBlocks` answers identically;
//! * the ARN an app block reports is `app-block/<name>` in the Region the request was
//!   signed for, under this emulator's account;
//! * an app block always reports a packaging type, `CUSTOM` for a request that names
//!   none, and the state it starts in follows its packaging and whether its source
//!   location names an application package;
//! * a create may carry a setup script only where the packaging needs one and a post
//!   setup script only where the packaging admits one, and a refused create persists
//!   nothing;
//! * a second create of a name that exists answers `ResourceAlreadyExistsException`;
//! * a read reports only stored app blocks, refuses an ARN that addresses none with
//!   `ResourceNotFoundException`, and pages every stored app block exactly once;
//! * every read that takes a `MaxResults` decides its page size through the one shared
//!   rule, so each refuses a page of fewer than one record with the same sentence;
//! * the tags a create persisted are the ones the tag operations report and add to.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::refusal_messages::{already_exists_sentence, not_found_sentence, refusal};
use crate::stack_fleet_lifecycle::Emulator;

/// The account every emulator ARN and every refusal sentence names.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The resource words AWS's refusal sentences use for an app block.
///
/// An app block is addressed by ARN wherever it is read, and AWS's not-found sentence for
/// one names the ARN the request gave under the capitalised resource word
/// ([`app_block_not_found_sentence`]); `APP_BLOCK_KIND` is the lower-case word the
/// duplicate-name sentence uses, where the app block is named by the name a create asked
/// for.
const APP_BLOCK_KIND: &str = "app block";
const APP_BLOCK_ARN_KIND: &str = "App block";

/// AWS's `ResourceNotFoundException` sentence for an app block this account does not
/// hold, which names the ARN the request addressed it with.
fn app_block_not_found_sentence(arn: &str) -> String {
    not_found_sentence(APP_BLOCK_ARN_KIND, arn)
}

/// The `PackagingType` values the model admits (`PACKAGING_TYPE_VALUES` in
/// `src/ops/types/shared_types.rs`, from `smithy_query(query="shape",
/// shape="PackagingType")`: CUSTOM | APPSTREAM2).
const CUSTOM: &str = "CUSTOM";
const APPSTREAM2: &str = "APPSTREAM2";

/// The `AppBlockState` values the model admits (`smithy_query(query="shape",
/// shape="AppBlockState")`: INACTIVE | ACTIVE).
const ACTIVE: &str = "ACTIVE";
const INACTIVE: &str = "INACTIVE";

/// The ARN an app block of this name carries: AppStream's `app-block` resource type, in
/// the Region the request was signed for.
fn app_block_arn(region: &str, name: &str) -> String {
    format!("arn:aws:appstream:{region}:{ACCOUNT}:app-block/{name}")
}

/// One row of the create table: a `CreateAppBlock` request and what the app block it
/// mints must report.
struct Row {
    case: String,
    name: String,
    /// The request, without its `Name`.
    request: Value,
    /// The `PackagingType` the created app block reports — the one the request named, or
    /// the service default for a request that named none.
    packaging_type: &'static str,
    /// The `AppBlockState` the created app block reports.
    state: &'static str,
}

/// A `ScriptDetails` as the request carries it: an S3 location, a run path, an optional
/// parameter string and a run timeout (`smithy_query(query="shape",
/// shape="ScriptDetails")`).
fn script(bucket: &str, key: &str, parameters: Option<&str>, timeout: i64) -> Value {
    let mut details = json!({
        "ScriptS3Location": { "S3Bucket": bucket, "S3Key": key },
        "ExecutablePath": "C:\\setup\\install.ps1",
        "TimeoutInSeconds": timeout,
    });
    if let Some(parameters) = parameters {
        details["ExecutableParameters"] = json!(parameters);
    }
    details
}

/// `CreateAppBlock` requests spanning the input class.
///
/// The rows differ along the packaging type (omitted, each value the model admits), the
/// form of the source S3 location (a flat key, a nested key, a dotted bucket, no key at
/// all), which optional text and script members are carried (none, empty, long,
/// parameterised), the number of tags, and the length and character classes of the name.
///
/// Member names and their constraints come from `smithy_query(query="operation",
/// operation="CreateAppBlock")`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "required members only, packaging type omitted".to_string(),
            name: "flat".to_string(),
            request: json!({
                "SourceS3Location": { "S3Bucket": "app-packages", "S3Key": "disks/office.vhd" },
                "SetupScriptDetails": script("app-packages", "scripts/setup.ps1", None, 60),
            }),
            packaging_type: CUSTOM,
            state: ACTIVE,
        },
        Row {
            case: "custom packaging named, every optional member carried, two tags"
                .to_string(),
            name: "custom.app-block_01".to_string(),
            request: json!({
                "PackagingType": CUSTOM,
                "Description": "Line-of-business applications packaged as a virtual hard disk",
                "DisplayName": "LOB apps",
                "SourceS3Location": { "S3Bucket": "app.packages-eu", "S3Key": "2026/q1/lob.vhd" },
                "SetupScriptDetails": script(
                    "app.packages-eu",
                    "scripts/2026/setup.ps1",
                    Some("-Mode Silent"),
                    3600,
                ),
                "Tags": { "Environment": "Test", "Owner": "platform" },
            }),
            packaging_type: CUSTOM,
            state: ACTIVE,
        },
        Row {
            // `Description` and `DisplayName` are @length 0.. on the request but 1.. on
            // the resource, so an empty value is an attribute the app block does not
            // have.
            case: "custom packaging, empty description and display name".to_string(),
            name: "0emptytext".to_string(),
            request: json!({
                "PackagingType": CUSTOM,
                "Description": "",
                "DisplayName": "",
                "SourceS3Location": { "S3Bucket": "app-packages", "S3Key": "disks/empty.vhd" },
                "SetupScriptDetails": script("app-packages", "scripts/setup.ps1", None, 1),
            }),
            packaging_type: CUSTOM,
            state: ACTIVE,
        },
        Row {
            // `S3Key` "refers to the VHD file" when an existing application package is
            // used, so this app block has one and needs no builder to produce it.
            case: "appstream2 packaging over an existing application package".to_string(),
            name: "appstream2-with-package".to_string(),
            request: json!({
                "PackagingType": APPSTREAM2,
                "DisplayName": "Managed apps",
                "SourceS3Location": { "S3Bucket": "app-packages", "S3Key": "disks/managed.vhd" },
                "PostSetupScriptDetails": script(
                    "app-packages",
                    "scripts/post-setup.ps1",
                    Some("/quiet"),
                    900,
                ),
                "Tags": { "Environment": "Prod" },
            }),
            packaging_type: APPSTREAM2,
            state: ACTIVE,
        },
        Row {
            // No `S3Key`: "if a new application package is required, then S3Key is not
            // required", and an app block with no package assigned is INACTIVE.
            case: "appstream2 packaging with no application package yet".to_string(),
            name: "appstream2-no-package".to_string(),
            request: json!({
                "PackagingType": APPSTREAM2,
                "SourceS3Location": { "S3Bucket": "app-packages" },
            }),
            packaging_type: APPSTREAM2,
            state: INACTIVE,
        },
        Row {
            // `Name`'s pattern is ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$ — 101 characters is
            // the longest name a create admits.
            case: "longest name a create admits, nested key".to_string(),
            name: format!(
                "A{}{}",
                "b_c.d-9".repeat(10),
                "012345678901234567890123456789"
            ),
            request: json!({
                "PackagingType": APPSTREAM2,
                "SourceS3Location": {
                    "S3Bucket": "app-packages",
                    "S3Key": "disks/nested/path/to/image.vhd",
                },
            }),
            packaging_type: APPSTREAM2,
            state: ACTIVE,
        },
    ]
}

/// The request a row sends: its members plus the `Name` it is created under.
fn create_request(row: &Row) -> Value {
    let mut request = row.request.clone();
    request["Name"] = json!(row.name);
    request
}

/// The `AppBlock` the row's create must report, bar its `CreatedTime`: every member the
/// request carried, rendered back, plus the identity and state the service decided.
fn expected_app_block(region: &str, row: &Row) -> Value {
    let mut expected = json!({
        "Name": row.name,
        "Arn": app_block_arn(region, &row.name),
        "SourceS3Location": row.request["SourceS3Location"].clone(),
        "PackagingType": row.packaging_type,
        "State": row.state,
    });
    for member in [
        "Description",
        "DisplayName",
        "SetupScriptDetails",
        "PostSetupScriptDetails",
    ] {
        match row.request.get(member) {
            // An empty text attribute is not one the resource reports.
            Some(Value::String(text)) if text.is_empty() => {}
            Some(value) => expected[member] = value.clone(),
            None => {}
        }
    }
    expected
}

/// Create the row's app block and assert what it reports, returning the `AppBlock` as the
/// create answered it (`CreatedTime` included).
async fn create_and_assert(emulator: &Emulator, region: &str, row: &Row) -> Value {
    let case = &row.case;
    let created = emulator
        .ok(case, "CreateAppBlock", create_request(row))
        .await;
    let app_block = created
        .get("AppBlock")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAppBlock must report an AppBlock"));

    let created_time = app_block.get("CreatedTime").and_then(Value::as_f64);
    assert!(
        created_time.is_some_and(|instant| instant > 0.0),
        "case {case:?}: the app block must report when it was created, got {app_block}"
    );
    assert!(
        app_block.get("AppBlockErrors").is_none(),
        "case {case:?}: a freshly created app block reports no errors, got {app_block}"
    );

    let mut without_created_time = app_block.clone();
    without_created_time
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {case:?}: the AppBlock must be an object"))
        .remove("CreatedTime");
    assert_eq!(
        without_created_time,
        expected_app_block(region, row),
        "case {case:?}: CreateAppBlock must report the app block it persisted"
    );
    app_block
}

/// The `AppBlocks` and `NextToken` of one `DescribeAppBlocks`.
async fn describe(emulator: &Emulator, case: &str, request: Value) -> (Vec<Value>, Option<Value>) {
    let payload = emulator.ok(case, "DescribeAppBlocks", request).await;
    let app_blocks = payload
        .get("AppBlocks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeAppBlocks must report AppBlocks"));
    (app_blocks, payload.get("NextToken").cloned())
}

#[tokio::test]
async fn create_app_block_persists_what_a_later_read_reports() {
    let region = "us-east-1";
    let emulator = Emulator::in_region(region);
    let mut stored: Vec<Value> = Vec::new();

    for row in rows() {
        let app_block = create_and_assert(&emulator, region, &row).await;
        let arn = app_block["Arn"].clone();

        // The round trip: the read renders the stored record, so it answers with exactly
        // what the create reported, `CreatedTime` included.
        let (described, next_token) = describe(
            &emulator,
            &row.case,
            json!({ "Arns": [arn] }),
        )
        .await;
        assert_eq!(
            described,
            vec![app_block.clone()],
            "case {:?}: DescribeAppBlocks must report the created app block",
            row.case
        );
        assert!(
            next_token.is_none(),
            "case {:?}: one app block fits in one page, got NextToken {next_token:?}",
            row.case
        );
        stored.push(app_block);
    }

    // A read that names no ARN asks about every app block the account holds.
    let (all, next_token) = describe(&emulator, "every stored app block", json!({})).await;
    assert!(
        next_token.is_none(),
        "case \"every stored app block\": the stored app blocks fit in one page, \
         got NextToken {next_token:?}"
    );
    let reported: BTreeSet<String> = all
        .iter()
        .map(|app_block| app_block["Name"].to_string())
        .collect();
    let expected: BTreeSet<String> = stored
        .iter()
        .map(|app_block| app_block["Name"].to_string())
        .collect();
    assert_eq!(
        reported, expected,
        "case \"every stored app block\": the read must report every stored app block, \
         got {all:?}"
    );
}

#[tokio::test]
async fn an_app_blocks_arn_names_the_region_the_request_was_signed_for() {
    // Two Regions, the same app block name: the ARN is what tells them apart, so a name
    // is not an identity on its own.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let emulator = Emulator::in_region(region);
        let row = Row {
            case: format!("region {region}"),
            name: "regional".to_string(),
            request: json!({
                "SourceS3Location": { "S3Bucket": "app-packages", "S3Key": "disks/app.vhd" },
                "SetupScriptDetails": script("app-packages", "scripts/setup.ps1", None, 30),
            }),
            packaging_type: CUSTOM,
            state: ACTIVE,
        };
        let app_block = create_and_assert(&emulator, region, &row).await;
        assert_eq!(
            app_block["Arn"].as_str(),
            Some(app_block_arn(region, &row.name).as_str()),
            "case {:?}: the ARN must name the request's Region",
            row.case
        );
    }
}

#[tokio::test]
async fn a_second_create_of_a_name_that_exists_already_exists() {
    let region = "us-east-1";
    let emulator = Emulator::in_region(region);

    for row in rows() {
        let app_block = create_and_assert(&emulator, region, &row).await;

        // The same name again, with a DIFFERENT body: the name is the identity, so what
        // the second request carries makes no difference.
        let mut again = create_request(&row);
        again["Description"] = json!("a second create of a name that is taken");
        let case = format!("{}: created twice", row.case);
        let message = refusal(
            &emulator,
            &case,
            "CreateAppBlock",
            again,
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
        assert_eq!(
            message,
            already_exists_sentence(APP_BLOCK_KIND, &row.name),
            "case {case:?}: the refusal must name the app block and the account"
        );

        // The refused create left the stored record alone.
        let (described, _) = describe(
            &emulator,
            &case,
            json!({ "Arns": [app_block["Arn"].clone()] }),
        )
        .await;
        assert_eq!(
            described,
            vec![app_block],
            "case {case:?}: the refused create must not change the stored app block"
        );
    }
}

#[tokio::test]
async fn a_create_carries_the_script_members_its_packaging_admits() {
    let emulator = Emulator::in_region("us-east-1");
    let source = json!({ "S3Bucket": "app-packages", "S3Key": "disks/app.vhd" });
    let setup = script("app-packages", "scripts/setup.ps1", None, 60);
    let post_setup = script("app-packages", "scripts/post-setup.ps1", None, 60);

    // The rules: `SetupScriptDetails` "must be provided for the CUSTOM PackagingType" and
    // `PostSetupScriptDetails` "can only be provided for the APPSTREAM2 PackagingType"
    // (`smithy_query(query="operation", operation="CreateAppBlock")`). A request naming no
    // packaging type is a CUSTOM one, so both rules reach it.
    let setup_required =
        "SetupScriptDetails must be provided for the CUSTOM PackagingType".to_string();
    let post_setup_refused = |packaging_type: &str| {
        format!(
            "PostSetupScriptDetails can only be provided for the APPSTREAM2 PackagingType, \
             was {packaging_type}"
        )
    };

    let refused: Vec<(&str, Value, String)> = vec![
        (
            "custom packaging, no setup script",
            json!({ "PackagingType": CUSTOM, "SourceS3Location": source.clone() }),
            setup_required.clone(),
        ),
        (
            "packaging omitted, no setup script",
            json!({ "SourceS3Location": source.clone() }),
            setup_required,
        ),
        (
            "custom packaging, a post setup script",
            json!({
                "PackagingType": CUSTOM,
                "SourceS3Location": source.clone(),
                "SetupScriptDetails": setup.clone(),
                "PostSetupScriptDetails": post_setup.clone(),
            }),
            post_setup_refused(CUSTOM),
        ),
        (
            "packaging omitted, a post setup script",
            json!({
                "SourceS3Location": source.clone(),
                "SetupScriptDetails": setup.clone(),
                "PostSetupScriptDetails": post_setup.clone(),
            }),
            post_setup_refused(CUSTOM),
        ),
    ];

    for (case, mut request, expected) in refused {
        let name = format!("refused-{}", case.replace([' ', ','], "-"));
        request["Name"] = json!(name);
        let message = refusal(
            &emulator,
            case,
            "CreateAppBlock",
            request,
            400,
            "ValidationException",
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: the refusal must state the rule the request broke"
        );

        // A refused create persists NOTHING: the ARN it would have minted addresses no
        // app block.
        let arn = app_block_arn("us-east-1", &name);
        let not_found = refusal(
            &emulator,
            case,
            "DescribeAppBlocks",
            json!({ "Arns": [arn.clone()] }),
            400,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            not_found,
            app_block_not_found_sentence(&arn),
            "case {case:?}: the refused create must have persisted nothing"
        );
    }

    // The same two members are admitted where the packaging calls for them.
    let admitted: Vec<(&str, Value, &str)> = vec![
        (
            "appstream2 packaging, no scripts",
            json!({ "PackagingType": APPSTREAM2, "SourceS3Location": source.clone() }),
            ACTIVE,
        ),
        (
            "appstream2 packaging, a post setup script",
            json!({
                "PackagingType": APPSTREAM2,
                "SourceS3Location": source.clone(),
                "PostSetupScriptDetails": post_setup,
            }),
            ACTIVE,
        ),
        (
            "appstream2 packaging, a setup script",
            json!({
                "PackagingType": APPSTREAM2,
                "SourceS3Location": source,
                "SetupScriptDetails": setup,
            }),
            ACTIVE,
        ),
    ];
    for (case, request, state) in admitted {
        let row = Row {
            case: case.to_string(),
            name: format!("admitted-{}", case.replace([' ', ','], "-")),
            request,
            packaging_type: APPSTREAM2,
            state,
        };
        create_and_assert(&emulator, "us-east-1", &row).await;
    }
}

#[tokio::test]
async fn a_read_refuses_an_arn_that_addresses_no_app_block() {
    let region = "us-east-1";
    let emulator = Emulator::in_region(region);
    let table = rows();
    let row = &table[0];
    let stored = create_and_assert(&emulator, region, row).await;

    // Each row makes one part of the ARN wrong, so the read must match the stored ARN in
    // full rather than only its name. The refusal names the ARN the request gave,
    // whatever is wrong with it: `Arns` addresses app blocks, so an ARN of another
    // Region, another account or another resource type addresses no app block here.
    let cases: Vec<(&str, String)> = vec![
        ("a name no app block carries", app_block_arn(region, "absent")),
        (
            "the stored name in another Region",
            app_block_arn("eu-west-2", &row.name),
        ),
        (
            "the stored name under another account",
            format!(
                "arn:aws:appstream:{region}:123456789012:app-block/{}",
                row.name
            ),
        ),
        (
            "the stored name as another resource type",
            format!("arn:aws:appstream:{region}:{ACCOUNT}:stack/{}", row.name),
        ),
    ];

    for (case, arn) in cases {
        let message = refusal(
            &emulator,
            case,
            "DescribeAppBlocks",
            json!({ "Arns": [arn.clone()] }),
            400,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message,
            app_block_not_found_sentence(&arn),
            "case {case:?}: the refusal must name the resource that was searched for"
        );
    }

    // One unknown ARN beside a stored one refuses the whole read: the request asked about
    // an app block that does not exist.
    let message = refusal(
        &emulator,
        "a stored ARN and an unknown one",
        "DescribeAppBlocks",
        json!({ "Arns": [stored["Arn"].clone(), app_block_arn(region, "absent")] }),
        400,
        "ResourceNotFoundException",
    )
    .await;
    assert_eq!(
        message,
        app_block_not_found_sentence(&app_block_arn(region, "absent")),
        "case \"a stored ARN and an unknown one\": the refusal must name the missing one"
    );
}

#[tokio::test]
async fn following_next_token_reports_every_app_block_exactly_once() {
    let region = "us-east-1";
    let emulator = Emulator::in_region(region);
    let names: Vec<String> = (0..7).map(|index| format!("paged-{index}")).collect();
    for name in &names {
        emulator
            .ok(
                name,
                "CreateAppBlock",
                json!({
                    "Name": name,
                    "PackagingType": APPSTREAM2,
                    "SourceS3Location": { "S3Bucket": "app-packages", "S3Key": "disks/app.vhd" },
                }),
            )
            .await;
    }

    // Every page size up to and past the number of stored records: each walk must report
    // every app block exactly once.
    for page_size in [1_usize, 2, 3, 7, 8, 50] {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<Value> = None;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = next_token.clone() {
                request["NextToken"] = token;
            }
            let (page, token) = describe(&emulator, &case, request).await;
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case {case:?}: a page holds between one and {page_size} records, \
                 got {}",
                page.len()
            );
            seen.extend(
                page.iter()
                    .map(|app_block| app_block["Name"].as_str().unwrap_or_default().to_string()),
            );
            next_token = token;
            if next_token.is_none() {
                break;
            }
        }
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: no app block may be reported twice, got {seen:?}"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<&String>>(),
            names.iter().collect::<BTreeSet<&String>>(),
            "case {case:?}: the walk must report every stored app block, got {seen:?}"
        );
    }

    // A page of fewer than one record is not a size the read can answer with, and a token
    // this service did not mint resumes nothing.
    for (case, request) in [
        ("MaxResults 0", json!({ "MaxResults": 0 })),
        ("MaxResults -1", json!({ "MaxResults": -1 })),
        ("a token this service did not mint", json!({ "NextToken": "not-a-token" })),
    ] {
        refusal(
            &emulator,
            case,
            "DescribeAppBlocks",
            request,
            400,
            "ValidationException",
        )
        .await;
    }
}

#[tokio::test]
async fn every_read_decides_its_page_size_by_the_same_rule() {
    // One case per read that takes a `MaxResults`: they share one page-size rule
    // (`crate::state::page_size`), so a page of fewer than one record is refused with the
    // same sentence whichever read is asked, and the size a request asks for is otherwise
    // honoured up to that read's own ceiling.
    let emulator = Emulator::in_region("us-east-1");
    let user_pool = json!({ "AuthenticationType": "USERPOOL" });
    // The values below one each read's own model admits: `DescribeUserStackAssociations`
    // targets `MaxResults` (`@range` 0..500), so the boundary validator refuses a negative
    // before the handler sees it, while the other three target a plain `Integer`
    // (`smithy_query(query="operation", operation="DescribeUserStackAssociations")`).
    let reads: Vec<(&str, Value, Vec<i64>)> = vec![
        ("DescribeAppBlocks", json!({}), vec![0, -1, -2_147_483_648]),
        (
            "DescribeAppBlockBuilders",
            json!({}),
            vec![0, -1, -2_147_483_648],
        ),
        ("DescribeUsers", user_pool.clone(), vec![0, -1, -2_147_483_648]),
        (
            "DescribeUserStackAssociations",
            json!({ "StackName": "any-stack" }),
            vec![0],
        ),
        (
            "DescribeUsageReportSubscriptions",
            json!({}),
            vec![0, -1, -2_147_483_648],
        ),
    ];

    for (operation, base, too_small_values) in reads {
        for too_small in too_small_values {
            let case = format!("{operation} with MaxResults {too_small}");
            let mut request = base.clone();
            request["MaxResults"] = json!(too_small);
            let message = refusal(
                &emulator,
                &case,
                operation,
                request,
                400,
                "ValidationException",
            )
            .await;
            assert_eq!(
                message,
                format!("MaxResults must be at least 1, was {too_small}"),
                "case {case:?}: every read must state the page-size rule the same way"
            );
        }

        // The smallest page a read can answer with is one, and every one of these reads
        // answers it rather than refusing it.
        let case = format!("{operation} with MaxResults 1");
        let mut request = base.clone();
        request["MaxResults"] = json!(1);
        emulator.ok(&case, operation, request).await;
    }
}

#[tokio::test]
async fn an_app_blocks_tags_are_the_ones_the_tag_operations_report() {
    let region = "us-east-1";
    let emulator = Emulator::in_region(region);

    for row in rows() {
        let app_block = create_and_assert(&emulator, region, &row).await;
        let arn = app_block["Arn"].clone();
        let created_tags = row
            .request
            .get("Tags")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let listed = emulator
            .ok(
                &row.case,
                "ListTagsForResource",
                json!({ "ResourceArn": arn.clone() }),
            )
            .await;
        assert_eq!(
            listed["Tags"], created_tags,
            "case {:?}: the read must report the tags the create persisted",
            row.case
        );

        // A tag write adds to that same map rather than to a second copy of it.
        emulator
            .ok(
                &row.case,
                "TagResource",
                json!({ "ResourceArn": arn.clone(), "Tags": { "Added": "later" } }),
            )
            .await;
        let listed = emulator
            .ok(
                &row.case,
                "ListTagsForResource",
                json!({ "ResourceArn": arn }),
            )
            .await;
        let mut expected = created_tags;
        expected["Added"] = json!("later");
        assert_eq!(
            listed["Tags"], expected,
            "case {:?}: a tag write must add to the app block's own tags",
            row.case
        );
    }
}
