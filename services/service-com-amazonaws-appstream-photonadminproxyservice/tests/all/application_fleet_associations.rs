// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's application/fleet associations over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * an application may stream on a fleet only when the fleet's own `Platform` is one of
//!   the application's `Platforms`: a fleet whose platform the application does not
//!   support is refused `InvalidParameterValueException` (400) carrying AWS's sentence —
//!   which names the ARN, the fleet, the fleet's platform and the application's platform
//!   list — and records NO association;
//! * a platform-compatible pair associates with HTTP 200 and the association is reported
//!   from either endpoint by `DescribeApplicationFleetAssociations`, which takes exactly
//!   one of them;
//! * "this is only supported for Elastic fleets", so a fleet of any other type is refused
//!   `OperationNotPermittedException` (400), and either endpoint this account does not
//!   hold is refused `ResourceNotFoundException` (400);
//! * an association holds the application: it may not be deleted until
//!   `DisassociateApplicationFleet` clears the edge.

use serde_json::{json, Value};

use crate::refusal_messages::refusal;
use crate::stack_fleet_lifecycle::{assert_error, Emulator};

/// The account every emulator ARN names.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region every case below signs for.
const REGION: &str = "us-east-1";

/// The app block every application below is created against.
const APP_BLOCK: &str = "association-app-block";

/// The `PlatformType` values the model admits, from
/// `smithy_query(query="shape", shape="PlatformType")`: the catalog a fleet's `Platform`
/// and an application's `Platforms` are drawn from.
const PLATFORM_TYPES: &[&str] = &[
    "WINDOWS",
    "WINDOWS_SERVER_2016",
    "WINDOWS_SERVER_2019",
    "WINDOWS_SERVER_2022",
    "WINDOWS_SERVER_2025",
    "AMAZON_LINUX2",
    "RHEL8",
    "ROCKY_LINUX8",
    "UBUNTU_PRO_2404",
];

/// The `FleetType` values the model admits (`FLEET_TYPE_VALUES` in
/// `src/ops/types/shared_types.rs`), and the one an application may be attached to.
const ELASTIC: &str = "ELASTIC";
const OTHER_FLEET_TYPES: &[&str] = &["ALWAYS_ON", "ON_DEMAND"];

/// The ARN an application of this name carries in [`REGION`].
fn application_arn(name: &str) -> String {
    format!("arn:aws:appstream:{REGION}:{ACCOUNT}:application/{name}")
}

/// The sentences the service states for the two refusals below, read from the crate so
/// the test tracks the wording rather than a copy of it.
fn platform_mismatch_sentence(
    application_arn: &str,
    fleet_name: &str,
    fleet_platform: &str,
    application_platforms: &[&str],
) -> String {
    let platforms: Vec<String> = application_platforms
        .iter()
        .map(ToString::to_string)
        .collect();
    service_com_amazonaws_appstream_photonadminproxyservice::state::application_platform_mismatch_message(
        application_arn,
        fleet_name,
        fleet_platform,
        &platforms,
    )
}

fn not_elastic_sentence(fleet_name: &str, fleet_type: &str) -> String {
    service_com_amazonaws_appstream_photonadminproxyservice::state::fleet_not_elastic_message(
        fleet_name, fleet_type,
    )
}

/// Create the app block every application is created against.
async fn create_app_block(emulator: &Emulator, case: &str) -> String {
    let created = emulator
        .ok(
            case,
            "CreateAppBlock",
            json!({
                "Name": APP_BLOCK,
                "PackagingType": "APPSTREAM2",
                "SourceS3Location": { "S3Bucket": "app-packages", "S3Key": "disks/app.vhd" },
            }),
        )
        .await;
    created
        .get("AppBlock")
        .and_then(|block| block.get("Arn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected the app block's ARN, got {created}"))
        .to_string()
}

/// Create an application supporting `platforms`, and answer its ARN.
async fn create_application(
    emulator: &Emulator,
    case: &str,
    name: &str,
    platforms: &[&str],
    app_block_arn: &str,
) -> String {
    let created = emulator
        .ok(
            case,
            "CreateApplication",
            json!({
                "Name": name,
                "AppBlockArn": app_block_arn,
                "Platforms": platforms,
                "InstanceFamilies": ["GENERAL_PURPOSE"],
                "IconS3Location": { "S3Bucket": "app-packages", "S3Key": "icons/app.png" },
                "LaunchPath": "C:\\app\\app.exe",
            }),
        )
        .await;
    created
        .get("Application")
        .and_then(|application| application.get("Arn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected the application's ARN, got {created}"))
        .to_string()
}

/// Create a fleet of `fleet_type` on `platform`.
async fn create_fleet(
    emulator: &Emulator,
    case: &str,
    name: &str,
    fleet_type: &str,
    platform: &str,
) {
    emulator
        .ok(
            case,
            "CreateFleet",
            json!({
                "Name": name,
                "InstanceType": "stream.standard.small",
                "FleetType": fleet_type,
                "Platform": platform,
                "MaxConcurrentSessions": 5,
            }),
        )
        .await;
}

/// The application ARNs a read of one endpoint reports.
async fn associated(emulator: &Emulator, case: &str, selector: Value) -> Vec<Value> {
    let payload = emulator
        .ok(case, "DescribeApplicationFleetAssociations", selector)
        .await;
    payload
        .get("ApplicationFleetAssociations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One row of the platform table: the platforms the application supports, the platform
/// the Elastic fleet runs, and whether that pair may be associated.
struct Row {
    case: &'static str,
    application_platforms: &'static [&'static str],
    fleet_platform: &'static str,
    compatible: bool,
}

/// The platform table. The rows differ along what the rule generalises over: how many
/// platforms the application supports (one, several, the four the model's `Platforms`
/// list admits at most, and none at all), which platform the fleet runs, and whether that
/// platform is among them — including a Linux application on a Windows fleet (the recorded
/// case) and a Windows application on a Linux fleet (its mirror), plus pairs the issue
/// never cited.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a Linux-only application on a Windows fleet",
            application_platforms: &["AMAZON_LINUX2"],
            fleet_platform: "WINDOWS_SERVER_2019",
            compatible: false,
        },
        Row {
            case: "a Windows-only application on a Linux fleet",
            application_platforms: &["WINDOWS_SERVER_2019"],
            fleet_platform: "AMAZON_LINUX2",
            compatible: false,
        },
        Row {
            case: "an application supporting another Windows release",
            application_platforms: &["WINDOWS_SERVER_2022"],
            fleet_platform: "WINDOWS_SERVER_2025",
            compatible: false,
        },
        Row {
            case: "an application supporting no platform at all",
            application_platforms: &[],
            fleet_platform: "RHEL8",
            compatible: false,
        },
        Row {
            case: "the fleet's platform among several the application supports",
            application_platforms: &["AMAZON_LINUX2", "RHEL8", "ROCKY_LINUX8"],
            fleet_platform: "RHEL8",
            compatible: true,
        },
        Row {
            case: "the fleet's platform the only one the application supports",
            application_platforms: &["WINDOWS_SERVER_2019"],
            fleet_platform: "WINDOWS_SERVER_2019",
            compatible: true,
        },
        Row {
            case: "a four-platform application, the most the model admits",
            application_platforms: &[
                "WINDOWS_SERVER_2016",
                "WINDOWS_SERVER_2019",
                "WINDOWS_SERVER_2022",
                "UBUNTU_PRO_2404",
            ],
            fleet_platform: "UBUNTU_PRO_2404",
            compatible: true,
        },
    ]
}

#[tokio::test]
async fn an_application_associates_only_with_a_fleet_whose_platform_it_supports() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::in_region(REGION);
        let app_block_arn = create_app_block(&emulator, case).await;
        let application = format!("platform-app-{index}");
        let fleet = format!("platform-fleet-{index}");
        let arn = create_application(
            &emulator,
            case,
            &application,
            row.application_platforms,
            &app_block_arn,
        )
        .await;
        create_fleet(&emulator, case, &fleet, ELASTIC, row.fleet_platform).await;

        let request = json!({ "FleetName": fleet, "ApplicationArn": arn });
        if row.compatible {
            let answer = emulator
                .ok(case, "AssociateApplicationFleet", request.clone())
                .await;
            assert_eq!(
                answer,
                json!({
                    "ApplicationFleetAssociation": {
                        "FleetName": fleet,
                        "ApplicationArn": arn,
                    }
                }),
                "case {case:?}: the result must carry the association that was recorded, \
                 got {answer}"
            );
        } else {
            let message = refusal(
                &emulator,
                case,
                "AssociateApplicationFleet",
                request.clone(),
                400,
                "InvalidParameterValueException",
            )
            .await;
            assert_eq!(
                message,
                platform_mismatch_sentence(
                    &arn,
                    &fleet,
                    row.fleet_platform,
                    row.application_platforms
                ),
                "case {case:?}: the refusal must state the fleet's platform and the \
                 application's, got {message:?}"
            );
        }

        // Either way, the read reports exactly the associations that were recorded —
        // from the fleet's side and from the application's.
        let expected: Vec<Value> = if row.compatible {
            vec![json!({ "FleetName": fleet, "ApplicationArn": arn })]
        } else {
            Vec::new()
        };
        for selector in [
            json!({ "FleetName": fleet }),
            json!({ "ApplicationArn": arn }),
        ] {
            let reported = associated(&emulator, case, selector.clone()).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: the read of {selector} must report the associations the \
                 store holds, got {reported:?}"
            );
        }

        // A repeat of an admitted association is answered the same way and records the
        // edge once, so associating again is not a second association.
        if row.compatible {
            emulator
                .ok(case, "AssociateApplicationFleet", request)
                .await;
            let reported = associated(&emulator, case, json!({ "FleetName": fleet })).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: the pair must be associated exactly once, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_platform_in_the_catalog_associates_with_its_own_fleet() {
    // Every `PlatformType` the model declares, each with an application that supports
    // exactly it: the rule holds for the whole catalog rather than for the platforms the
    // issue cited, and the NEXT platform in the catalog is the mismatch of each row.
    let case = "the whole platform catalog";
    let emulator = Emulator::in_region(REGION);
    let app_block_arn = create_app_block(&emulator, case).await;

    for (index, platform) in PLATFORM_TYPES.iter().enumerate() {
        let application = format!("catalog-app-{index}");
        let fleet = format!("catalog-fleet-{index}");
        let arn =
            create_application(&emulator, case, &application, &[platform], &app_block_arn).await;
        create_fleet(&emulator, case, &fleet, ELASTIC, platform).await;

        emulator
            .ok(
                case,
                "AssociateApplicationFleet",
                json!({ "FleetName": fleet, "ApplicationArn": arn }),
            )
            .await;
        let reported = associated(&emulator, case, json!({ "ApplicationArn": arn })).await;
        assert_eq!(
            reported,
            vec![json!({ "FleetName": fleet, "ApplicationArn": arn })],
            "case {case:?} ({platform}): the association must be reported, got {reported:?}"
        );

        // The next platform in the catalog is one this application does not support, so a
        // fleet running it may not stream the application.
        let other = PLATFORM_TYPES[(index + 1) % PLATFORM_TYPES.len()];
        let other_fleet = format!("catalog-other-fleet-{index}");
        create_fleet(&emulator, case, &other_fleet, ELASTIC, other).await;
        let message = refusal(
            &emulator,
            case,
            "AssociateApplicationFleet",
            json!({ "FleetName": other_fleet, "ApplicationArn": arn }),
            400,
            "InvalidParameterValueException",
        )
        .await;
        assert_eq!(
            message,
            platform_mismatch_sentence(&arn, &other_fleet, other, &[platform]),
            "case {case:?} ({platform} application on a {other} fleet): unexpected \
             sentence {message:?}"
        );
        let reported = associated(&emulator, case, json!({ "FleetName": other_fleet })).await;
        assert!(
            reported.is_empty(),
            "case {case:?} ({platform}): the refused association must record nothing, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn only_an_elastic_fleet_streams_an_application() {
    // Rows: the fleet types that are not Elastic. Each runs the platform the application
    // supports, so the only thing refusing the association is the fleet's type.
    let case = "a fleet that is not Elastic";
    let emulator = Emulator::in_region(REGION);
    let app_block_arn = create_app_block(&emulator, case).await;
    let arn = create_application(
        &emulator,
        case,
        "fleet-type-app",
        &["WINDOWS_SERVER_2019"],
        &app_block_arn,
    )
    .await;

    for fleet_type in OTHER_FLEET_TYPES {
        let fleet = format!("fleet-type-{}", fleet_type.to_lowercase());
        create_fleet(&emulator, case, &fleet, fleet_type, "WINDOWS_SERVER_2019").await;
        let message = refusal(
            &emulator,
            case,
            "AssociateApplicationFleet",
            json!({ "FleetName": fleet, "ApplicationArn": arn }),
            400,
            "OperationNotPermittedException",
        )
        .await;
        assert_eq!(
            message,
            not_elastic_sentence(&fleet, fleet_type),
            "case {case:?} ({fleet_type}): the refusal must state the Elastic-only rule"
        );
        let reported = associated(&emulator, case, json!({ "FleetName": fleet })).await;
        assert!(
            reported.is_empty(),
            "case {case:?} ({fleet_type}): nothing may be recorded, got {reported:?}"
        );
    }

    // The positive control: an Elastic fleet on the same platform associates.
    create_fleet(&emulator, case, "fleet-type-elastic", ELASTIC, "WINDOWS_SERVER_2019").await;
    emulator
        .ok(
            case,
            "AssociateApplicationFleet",
            json!({ "FleetName": "fleet-type-elastic", "ApplicationArn": arn }),
        )
        .await;
}

#[tokio::test]
async fn a_fleet_that_publishes_no_platform_supports_no_application() {
    // `Platform` is optional on the wire, and a fleet that publishes none supports none
    // of an application's platforms: the association is refused with the same sentence,
    // naming the platform the fleet reports — nothing.
    let case = "a fleet with no platform";
    let emulator = Emulator::in_region(REGION);
    let app_block_arn = create_app_block(&emulator, case).await;
    let arn = create_application(
        &emulator,
        case,
        "platformless-app",
        &["WINDOWS_SERVER_2019"],
        &app_block_arn,
    )
    .await;
    emulator
        .ok(
            case,
            "CreateFleet",
            json!({
                "Name": "platformless-fleet",
                "InstanceType": "stream.standard.small",
                "FleetType": ELASTIC,
                "MaxConcurrentSessions": 5,
            }),
        )
        .await;

    let message = refusal(
        &emulator,
        case,
        "AssociateApplicationFleet",
        json!({ "FleetName": "platformless-fleet", "ApplicationArn": arn }),
        400,
        "InvalidParameterValueException",
    )
    .await;
    assert_eq!(
        message,
        platform_mismatch_sentence(&arn, "platformless-fleet", "", &["WINDOWS_SERVER_2019"]),
        "case {case:?}: the refusal must name the platforms on both sides, got {message:?}"
    );
    let reported = associated(&emulator, case, json!({ "ApplicationArn": arn })).await;
    assert!(
        reported.is_empty(),
        "case {case:?}: the refused association must record nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn an_endpoint_this_account_does_not_hold_is_not_found() {
    let case = "an endpoint that does not exist";
    let emulator = Emulator::in_region(REGION);
    let app_block_arn = create_app_block(&emulator, case).await;
    let arn = create_application(
        &emulator,
        case,
        "stored-app",
        &["WINDOWS_SERVER_2019"],
        &app_block_arn,
    )
    .await;
    create_fleet(&emulator, case, "stored-fleet", ELASTIC, "WINDOWS_SERVER_2019").await;

    // Rows: each makes exactly one endpoint the missing one — a fleet name nothing was
    // created under, and ARNs wrong in the name, the Region, the account and the resource
    // type, so the lookup matches the stored ARN in full.
    for (label, request) in [
        (
            "a fleet name nothing was created under",
            json!({ "FleetName": "absent-fleet", "ApplicationArn": arn }),
        ),
        (
            "an application name nothing was created under",
            json!({ "FleetName": "stored-fleet", "ApplicationArn": application_arn("absent") }),
        ),
        (
            "the stored application in another Region",
            json!({
                "FleetName": "stored-fleet",
                "ApplicationArn":
                    format!("arn:aws:appstream:eu-west-1:{ACCOUNT}:application/stored-app"),
            }),
        ),
        (
            "the stored application under another account",
            json!({
                "FleetName": "stored-fleet",
                "ApplicationArn":
                    format!("arn:aws:appstream:{REGION}:210987654321:application/stored-app"),
            }),
        ),
        (
            "the stored name as another resource type",
            json!({
                "FleetName": "stored-fleet",
                "ApplicationArn":
                    format!("arn:aws:appstream:{REGION}:{ACCOUNT}:app-block/stored-app"),
            }),
        ),
    ] {
        assert_error(
            &emulator,
            label,
            "AssociateApplicationFleet",
            request,
            400,
            "ResourceNotFoundException",
        )
        .await;
        let reported = associated(&emulator, label, json!({ "FleetName": "stored-fleet" })).await;
        assert!(
            reported.is_empty(),
            "case {label:?}: a refused association records nothing, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_read_takes_exactly_one_endpoint() {
    let case = "the read's selector";
    let emulator = Emulator::in_region(REGION);
    let app_block_arn = create_app_block(&emulator, case).await;
    let arn = create_application(
        &emulator,
        case,
        "selector-app",
        &["WINDOWS_SERVER_2019"],
        &app_block_arn,
    )
    .await;

    // "Either ApplicationArn or FleetName must be specified": neither is a missing
    // parameter and both an incorrect combination, which is the one modelled error.
    for (label, selector) in [
        ("neither endpoint", json!({})),
        (
            "both endpoints",
            json!({ "FleetName": "selector-fleet", "ApplicationArn": arn }),
        ),
    ] {
        let message = refusal(
            &emulator,
            label,
            "DescribeApplicationFleetAssociations",
            selector,
            400,
            "InvalidParameterCombinationException",
        )
        .await;
        assert_eq!(
            message,
            service_com_amazonaws_appstream_photonadminproxyservice::state::application_fleet_association_selector_message(),
            "case {label:?}: the refusal must state which endpoints a read takes"
        );
    }
}

#[tokio::test]
async fn an_association_holds_the_application_until_it_is_cleared() {
    // Rows: how many fleets stream the one application. Each association holds it, so the
    // delete is refused until every edge is cleared.
    for fleets in 1..=3_usize {
        let case = format!("{fleets} fleet(s) streaming one application");
        let emulator = Emulator::in_region(REGION);
        let app_block_arn = create_app_block(&emulator, &case).await;
        let arn = create_application(
            &emulator,
            &case,
            "held-app",
            &["WINDOWS_SERVER_2019"],
            &app_block_arn,
        )
        .await;

        let names: Vec<String> = (0..fleets).map(|index| format!("held-fleet-{index}")).collect();
        for fleet in &names {
            create_fleet(&emulator, &case, fleet, ELASTIC, "WINDOWS_SERVER_2019").await;
            emulator
                .ok(
                    &case,
                    "AssociateApplicationFleet",
                    json!({ "FleetName": fleet, "ApplicationArn": arn }),
                )
                .await;
        }

        for (cleared, fleet) in names.iter().enumerate() {
            let message = refusal(
                &emulator,
                &case,
                "DeleteApplication",
                json!({ "Name": "held-app" }),
                400,
                "ResourceInUseException",
            )
            .await;
            assert_eq!(
                message,
                service_com_amazonaws_appstream_photonadminproxyservice::state::application_in_use_message("held-app"),
                "case {case:?}: with {} association(s) left the delete must be refused",
                fleets - cleared
            );
            emulator
                .ok(
                    &case,
                    "DisassociateApplicationFleet",
                    json!({ "FleetName": fleet, "ApplicationArn": arn }),
                )
                .await;
            let reported = associated(&emulator, &case, json!({ "FleetName": fleet })).await;
            assert!(
                reported.is_empty(),
                "case {case:?}: the cleared edge must be gone, got {reported:?}"
            );
        }

        // Nothing holds it now, so the application can be deleted and the app block with
        // it.
        emulator
            .ok(&case, "DeleteApplication", json!({ "Name": "held-app" }))
            .await;
        emulator
            .ok(&case, "DeleteAppBlock", json!({ "Name": APP_BLOCK }))
            .await;
    }
}
