// SPDX-License-Identifier: CC0-1.0

//! The plan a `StartInstance` is asked to bring an instance up on, driven over
//! the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `attributeUpdates.bundleId` is the plan the instance
//! runs on from this start onward — a plan the service publishes, or `auto` for
//! the newer-generation equivalent of the plan it already runs on — it is
//! applied only while the instance is stopped, and every later read reports that
//! plan and the hardware it carries.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, wait_for_state, Emulator};

/// The blueprint every instance in this suite is created on, an id of the
/// service's published catalog (`GetBlueprints`).
const BLUEPRINT_ID: &str = "amazon_linux_2023";

/// The zone every instance in this suite is created in.
const ZONE: &str = "us-east-1a";

/// The plan every instance in this suite is created on, and the one a case moves
/// it to: ids of the service's published plan table (`GetBundles`).
const CREATED_BUNDLE_ID: &str = "nano_3_0";
const LARGER_BUNDLE_ID: &str = "small_3_0";

/// The value `attributeUpdates.bundleId` carries to ask for the
/// newer-generation equivalent of the plan the instance already runs on, from
/// the modelled `InstanceAttributeUpdates$bundleId` documentation.
const AUTO: &str = "auto";

/// Create the instance `name` on `bundle_id`, wait until it is running, then
/// stop it — the state the documented rule requires for a bundle update.
async fn create_stopped_instance(emulator: &Emulator, name: &str, bundle_id: &str) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": ZONE,
                "blueprintId": BLUEPRINT_ID,
                "bundleId": bundle_id,
            }),
        )
        .await;
    wait_for_state(emulator, name, "running").await;
    emulator
        .ok("StopInstance", json!({"instanceName": name}))
        .await;
    wait_for_state(emulator, name, "stopped").await;
}

/// The instance `name`, as `GetInstance` reports it.
async fn instance(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetInstance", json!({"instanceName": name})).await["instance"].clone()
}

/// Start the instance `name`, asking for the plan `bundle_id` when one is
/// named.
async fn start(emulator: &Emulator, name: &str, bundle_id: Option<&str>) -> (u16, Value) {
    let mut body = json!({"instanceName": name});
    if let Some(bundle_id) = bundle_id {
        body["attributeUpdates"] = json!({"bundleId": bundle_id});
    }
    emulator.call("StartInstance", body).await
}

/// One `StartInstance` request's `attributeUpdates`, and the plan the rule says
/// the instance then runs on. The rows differ along what the rule generalises
/// over: no attribute update at all, a plan named outright, and the `auto` that
/// resolves to the equivalent of the plan the instance already runs on.
struct Row {
    case: &'static str,
    requested: Option<&'static str>,
    expected: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "no attribute update",
        requested: None,
        expected: CREATED_BUNDLE_ID,
    },
    Row {
        case: "a plan named outright",
        requested: Some(LARGER_BUNDLE_ID),
        expected: LARGER_BUNDLE_ID,
    },
    Row {
        case: "auto, on an instance already at the published generation",
        requested: Some(AUTO),
        expected: CREATED_BUNDLE_ID,
    },
];

/// RULE: a start that names a plan brings the instance up on that plan, so
/// every later read reports it and the hardware it carries, and a start that
/// names none leaves the plan the instance already ran on.
#[tokio::test]
async fn a_start_brings_the_instance_up_on_the_plan_it_names() {
    let emulator = Emulator::new();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("bundle-start-{index}");
        create_stopped_instance(&emulator, &name, CREATED_BUNDLE_ID).await;
        let before = instance(&emulator, &name).await;

        let (status, answered) = start(&emulator, &name, row.requested).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {answered:?}"
        );
        assert_eq!(
            answered["operations"][0]["operationType"].as_str(),
            Some("StartInstance"),
            "case {case}: expected one start log entry, got {answered:?}"
        );

        let after = wait_for_state(&emulator, &name, "running").await;
        assert_eq!(
            after["name"].as_str(),
            Some("running"),
            "case {case}: the instance is serving again, got {after:?}"
        );
        let reported = instance(&emulator, &name).await;
        assert_eq!(
            reported["bundleId"].as_str(),
            Some(row.expected),
            "case {case}: expected the plan the start asked for, got {reported:?}"
        );
        // The hardware a read reports is the one that plan carries, so a plan
        // change is more than a relabelled id.
        let (ram_before, cpu_before) = (
            before["hardware"]["ramSizeInGb"].as_f64(),
            before["hardware"]["cpuCount"].as_i64(),
        );
        let (ram_after, cpu_after) = (
            reported["hardware"]["ramSizeInGb"].as_f64(),
            reported["hardware"]["cpuCount"].as_i64(),
        );
        if row.expected == CREATED_BUNDLE_ID {
            assert_eq!(
                (ram_after, cpu_after),
                (ram_before, cpu_before),
                "case {case}: the hardware of the unchanged plan is unchanged, got {reported:?}"
            );
        } else {
            assert!(
                ram_after > ram_before,
                "case {case}: expected the hardware of the larger plan, got {reported:?}"
            );
        }
    }
}

/// RULE: a plan the published table does not hold, and a plan update asked of an
/// instance that is not stopped, are refused with the modelled failure for that
/// rule and change nothing — the instance keeps the plan it ran on.
#[tokio::test]
async fn a_plan_update_the_rules_do_not_allow_changes_nothing() {
    let emulator = Emulator::new();

    // A plan no published size token names is refused as input.
    create_stopped_instance(&emulator, "bundle-refusal", CREATED_BUNDLE_ID).await;
    let (status, body) = start(&emulator, "bundle-refusal", Some("invented_9_0")).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInputException".to_string()),
        "case a plan the catalog does not hold: expected the modelled input refusal, got \
         {status} {body:?}"
    );
    let reported = instance(&emulator, "bundle-refusal").await;
    assert_eq!(
        (
            reported["bundleId"].as_str(),
            reported["state"]["name"].as_str()
        ),
        (Some(CREATED_BUNDLE_ID), Some("stopped")),
        "case a plan the catalog does not hold: the refusal left the instance alone, got \
         {reported:?}"
    );

    // "The instance must be in the stopped state to update its bundle": a
    // running instance's plan is not updated by a start.
    create_stopped_instance(&emulator, "bundle-running", CREATED_BUNDLE_ID).await;
    let (status, answered) = start(&emulator, "bundle-running", None).await;
    assert_eq!(
        status, 200,
        "case bringing the instance up first: expected HTTP 200, got {answered:?}"
    );
    wait_for_state(&emulator, "bundle-running", "running").await;
    let (status, body) = start(&emulator, "bundle-running", Some(LARGER_BUNDLE_ID)).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "OperationFailureException".to_string()),
        "case a plan update on a running instance: expected the modelled operation failure, got \
         {status} {body:?}"
    );
    let reported = instance(&emulator, "bundle-running").await;
    assert_eq!(
        reported["bundleId"].as_str(),
        Some(CREATED_BUNDLE_ID),
        "case a plan update on a running instance: the refusal left the plan alone, got \
         {reported:?}"
    );

    // An instance the account does not hold is the not-found envelope, whether
    // or not the request asks for a plan update.
    for requested in [None, Some(LARGER_BUNDLE_ID)] {
        let (status, body) = start(&emulator, "never-created", requested).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "NotFoundException".to_string()),
            "case starting an instance nothing created ({requested:?}): expected the not-found \
             envelope, got {status} {body:?}"
        );
    }
}
