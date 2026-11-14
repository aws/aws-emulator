// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyVolume`, driven over the wire this service really
//! speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.ModifyVolume`).
//!
//! The rule under test, stated once and asserted over a SET of inputs:
//!
//! > `ModifyVolume` resolves its `VolumeId` against the volumes the signed Region
//! > holds and records ONE asynchronous modification of that volume, whose target is
//! > per member the value the call names and, for a member the call leaves out, the
//! > value the volume keeps — except the performance members, which belong to the
//! > volume type that provisions them and so take the target type's own values when
//! > the type changes. That target is what the call reports, and what the volume's
//! > configuration becomes once the modification finishes. A call that names none of
//! > the modifiable members, or whose target is one the target volume type does not
//! > accept (a size below the volume's current size or outside the type's range, a
//! > performance member the type takes no value for or one outside its range), is
//! > refused with the modelled `ValidationException` (HTTP 400) naming the offending
//! > member and records nothing; a second modification while one is still running, or
//! > one past the rolling limit the service keeps, is refused with
//! > `VolumeModificationRateExceededException` (HTTP 429); a retry carrying the same
//! > `ClientToken` replays that one modification, and a reuse of it for different
//! > parameters is `ConflictException` (HTTP 409).
//!
//! The rows below differ along what that rule generalises over: which member the call
//! names, how many of them, which volume type is the source and which the target,
//! whether the volume has a size of its own at all, and which side of a published
//! bound a value falls on (each bound and the value one past it).
//!
//! Nothing here can pass against a canned answer: every id comes back out of
//! `CreateVolume`, every target is read back out of the service's own store, and the
//! configuration a finished modification leaves behind is re-observed through the wire
//! (a later `ModifyVolume` is refused, or accepted, only for the configuration the
//! volume has by then).

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::volume_lifecycle::{create_volume, simple_create};
use crate::workspace_instance_lifecycle::{
    call, call_in_region, error_code, harness, ok, DEFAULT_TEST_REGION,
};

/// The `ValidationExceptionReason` a bad member is reported with, as the modelled enum
/// value (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// Enough slack past a stored deadline that the clock has certainly passed it.
/// `tokio::time::sleep` never wakes early, so this only has to cover timer
/// resolution.
const SLACK: Duration = Duration::from_millis(250);

/// The volume `id` as Region `region`'s store holds it, or `None` for a volume that
/// Region does not hold — the engine the router was built from, read the one legal way.
///
/// `pub(crate)` with the wire helpers of the other suites: the sibling
/// `volume_attachment` suite reads the stored record through this one rather than
/// opening the store a second way, and needs the `None` (a record a delete removed) and
/// the Region (its rows run in more than one).
pub(crate) fn stored_volume_in(
    engine: &engine::InMemoryEngine,
    region: &str,
    id: &str,
) -> Option<wsinst::state::VolumeRecord> {
    wsinst::state::service_state(engine)
        .expect("the service's store is reachable from the engine the router was built from")
        .lock()
        .volume(region, id)
        .cloned()
}

/// [`stored_volume_in`] for this suite's own Region, where every row expects the record
/// to be there.
fn stored_volume(engine: &engine::InMemoryEngine, id: &str) -> wsinst::state::VolumeRecord {
    stored_volume_in(engine, DEFAULT_TEST_REGION, id)
        .unwrap_or_else(|| panic!("the store holds no volume {id:?} in {DEFAULT_TEST_REGION}"))
}

/// Drive one request whose only purpose is to let the service materialise what the
/// clock owes.
///
/// Every write path of this service settles the clock-due work before it reads, so a
/// call this test does not otherwise care about is how the store is brought up to date
/// WITHOUT the test applying the clock itself. A `ModifyVolume` for an id nobody
/// minted settles first and is then refused, which is the refusal asserted here.
async fn settle_through_the_wire(router: &MultiServiceRouter) {
    let (status, payload) = call(
        router,
        "ModifyVolume",
        json!({ "VolumeId": "vol-0", "SizeInGB": 1 }),
    )
    .await;
    assert_eq!(
        status,
        wsinst::state::RESOURCE_NOT_FOUND_STATUS,
        "case settling call: expected the service's not-found status, got {status} with {payload}"
    );
}

/// `ModifyVolume` for `id`, with `body`'s members beside the required `VolumeId`.
async fn modify(router: &MultiServiceRouter, id: &str, body: &Value) -> (u16, Value) {
    let mut request = body.clone();
    request["VolumeId"] = json!(id);
    call(router, "ModifyVolume", request).await
}

/// One row of the target-resolution table: the volume to create, the modification to
/// ask for, and the target each output member must carry.
struct TargetRow {
    case: &'static str,
    create: Value,
    modify: Value,
    target_size: Option<i64>,
    target_volume_type: &'static str,
    target_iops: Option<i64>,
    target_throughput: Option<i64>,
}

/// The modifications the resolution rule is asserted over: one row per dimension it
/// generalises over — each modifiable member on its own and all of them at once, a
/// retained type and a changed one in both directions (to a type that provisions
/// IOPS and throughput, and to one that provisions neither), a volume with no size of
/// its own, and the published bounds of the sizes involved.
///
/// Every type of the modelled `VolumeTypeEnum` appears as a source or a target; the
/// per-type ranges the rows sit inside are the service's own published table
/// (`wsinst::state::VOLUME_TYPE_SPECS`).
fn target_rows() -> Vec<TargetRow> {
    vec![
        TargetRow {
            case: "SizeInGB alone, with the type and its members retained",
            create: json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 4 }),
            modify: json!({ "SizeInGB": 40 }),
            target_size: Some(40),
            target_volume_type: "gp2",
            target_iops: None,
            target_throughput: None,
        },
        TargetRow {
            case: "SizeInGB at the top of the modelled range",
            create: json!({ "AvailabilityZone": "us-east-1b", "SizeInGB": 4 }),
            modify: json!({ "SizeInGB": 16384 }),
            target_size: Some(16384),
            target_volume_type: "gp2",
            target_iops: None,
            target_throughput: None,
        },
        TargetRow {
            case: "VolumeType alone, to a type that provisions IOPS and throughput",
            create: json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 8 }),
            modify: json!({ "VolumeType": "gp3" }),
            target_size: Some(8),
            target_volume_type: "gp3",
            target_iops: Some(3000),
            target_throughput: Some(125),
        },
        TargetRow {
            case: "VolumeType alone, to a type that provisions IOPS only",
            create: json!({ "AvailabilityZone": "us-east-1c", "SizeInGB": 100 }),
            modify: json!({ "VolumeType": "io1" }),
            target_size: Some(100),
            target_volume_type: "io1",
            target_iops: Some(3000),
            target_throughput: None,
        },
        TargetRow {
            case: "VolumeType alone, to a type that provisions neither",
            create: json!({
                "AvailabilityZone": "us-east-1a",
                "VolumeType": "gp3",
                "SizeInGB": 125,
                "Iops": 5000,
                "Throughput": 300,
            }),
            modify: json!({ "VolumeType": "gp2" }),
            target_size: Some(125),
            target_volume_type: "gp2",
            target_iops: None,
            target_throughput: None,
        },
        TargetRow {
            case: "Iops alone, with the size and throughput retained",
            create: json!({
                "AvailabilityZone": "us-east-1b",
                "VolumeType": "gp3",
                "SizeInGB": 125,
                "Iops": 3000,
                "Throughput": 200,
            }),
            modify: json!({ "Iops": 4000 }),
            target_size: Some(125),
            target_volume_type: "gp3",
            target_iops: Some(4000),
            target_throughput: Some(200),
        },
        TargetRow {
            case: "Throughput alone, at the top of its published range",
            create: json!({
                "AvailabilityZone": "us-east-1b",
                "VolumeType": "gp3",
                "SizeInGB": 125,
                "Iops": 3000,
                "Throughput": 200,
            }),
            modify: json!({ "Throughput": 2000 }),
            target_size: Some(125),
            target_volume_type: "gp3",
            target_iops: Some(3000),
            target_throughput: Some(2000),
        },
        TargetRow {
            case: "every modifiable member at once",
            create: json!({ "AvailabilityZone": "us-east-1c", "SizeInGB": 10 }),
            modify: json!({
                "SizeInGB": 500,
                "VolumeType": "gp3",
                "Iops": 10000,
                "Throughput": 500,
            }),
            target_size: Some(500),
            target_volume_type: "gp3",
            target_iops: Some(10000),
            target_throughput: Some(500),
        },
        TargetRow {
            case: "a volume with no size of its own, restored from a snapshot",
            create: json!({
                "AvailabilityZone": "us-east-1b",
                "SnapshotId": "snap-0123456789abcdef",
            }),
            modify: json!({ "VolumeType": "gp3" }),
            target_size: None,
            target_volume_type: "gp3",
            target_iops: Some(3000),
            target_throughput: Some(125),
        },
        TargetRow {
            case: "between two types that provision neither, at their shared floor",
            create: json!({
                "AvailabilityZone": "us-east-1a",
                "VolumeType": "sc1",
                "SizeInGB": 125,
            }),
            modify: json!({ "VolumeType": "st1" }),
            target_size: Some(125),
            target_volume_type: "st1",
            target_iops: None,
            target_throughput: None,
        },
        TargetRow {
            case: "SizeInGB on a type that requires IOPS, which are retained",
            create: json!({
                "AvailabilityZone": "us-east-1c",
                "VolumeType": "io2",
                "SizeInGB": 4,
                "Iops": 100,
            }),
            modify: json!({ "SizeInGB": 8 }),
            target_size: Some(8),
            target_volume_type: "io2",
            target_iops: Some(100),
            target_throughput: None,
        },
        TargetRow {
            case: "SizeInGB at the published ceiling of its own type",
            create: json!({
                "AvailabilityZone": "us-east-1a",
                "VolumeType": "standard",
                "SizeInGB": 100,
            }),
            modify: json!({ "SizeInGB": 1024 }),
            target_size: Some(1024),
            target_volume_type: "standard",
            target_iops: None,
            target_throughput: None,
        },
    ]
}

#[tokio::test]
async fn the_target_is_resolved_per_member_and_becomes_the_volumes_configuration() {
    let (router, engine) = harness();
    let rows = target_rows();

    // Every published volume type takes part, so no row set can pass while a type is
    // left without rules: the list is the model's own
    // (`smithy_query(query="shape",shape="VolumeTypeEnum")`).
    for volume_type in wsinst::ops::types::VOLUME_TYPE_ENUM_VALUES {
        assert!(
            rows.iter().any(|row| {
                row.target_volume_type == *volume_type
                    || row.create["VolumeType"].as_str() == Some(volume_type)
            }),
            "case catalog: {volume_type:?} takes part in no row, so its rules are untested"
        );
    }

    let mut modified: Vec<(&TargetRow, String)> = Vec::new();
    for row in &rows {
        let id = create_volume(&router, row.create.clone()).await;
        let (status, payload) = modify(&router, &id, &row.modify).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {payload}",
            row.case
        );
        assert_eq!(
            payload["VolumeId"].as_str(),
            Some(id.as_str()),
            "case {:?}: expected the response to name the modified volume, got {payload}",
            row.case
        );
        assert_eq!(
            payload["TargetSize"].as_i64(),
            row.target_size,
            "case {:?}: expected TargetSize {:?}, got {payload}",
            row.case,
            row.target_size
        );
        assert_eq!(
            payload["TargetVolumeType"].as_str(),
            Some(row.target_volume_type),
            "case {:?}: expected TargetVolumeType {:?}, got {payload}",
            row.case,
            row.target_volume_type
        );
        assert_eq!(
            payload["TargetIops"].as_i64(),
            row.target_iops,
            "case {:?}: expected TargetIops {:?}, got {payload}",
            row.case,
            row.target_iops
        );
        assert_eq!(
            payload["TargetThroughput"].as_i64(),
            row.target_throughput,
            "case {:?}: expected TargetThroughput {:?}, got {payload}",
            row.case,
            row.target_throughput
        );

        // ONE modification was recorded for the volume, carrying exactly that target:
        // the response was read out of the store, not built from the request.
        let stored = stored_volume(&engine, &id);
        assert_eq!(
            stored.modifications.len(),
            1,
            "case {:?}: expected one recorded modification, got {:?}",
            row.case,
            stored.modifications
        );
        let recorded = &stored.modifications[0];
        assert_eq!(
            (
                recorded.target_size_in_gb.map(i64::from),
                recorded.target_volume_type.as_str(),
                recorded.target_iops.map(i64::from),
                recorded.target_throughput.map(i64::from),
            ),
            (
                row.target_size,
                row.target_volume_type,
                row.target_iops,
                row.target_throughput,
            ),
            "case {:?}: the recorded modification must carry the reported target, got {recorded:?}",
            row.case
        );
        // The volume itself is unchanged while the modification runs: a modification is
        // asynchronous, so its target is not yet the configuration.
        assert!(
            recorded.completes_at > std::time::Instant::now(),
            "case {:?}: the modification must still be running right after the call",
            row.case
        );
        modified.push((row, id));
    }

    // Past the span, the target IS the configuration — materialised the way every
    // other clock-due transition of this service is, through the next call.
    tokio::time::sleep(wsinst::state::VOLUME_MODIFICATION_SPAN + SLACK).await;
    settle_through_the_wire(&router).await;
    for (row, id) in &modified {
        let stored = stored_volume(&engine, id);
        assert_eq!(
            (
                stored.size_in_gb.map(i64::from),
                stored.effective_volume_type(),
                stored.iops.map(i64::from),
                stored.throughput.map(i64::from),
            ),
            (
                row.target_size,
                row.target_volume_type,
                row.target_iops,
                row.target_throughput,
            ),
            "case {:?}: expected the finished modification's target to be the volume's \
             configuration, got {stored:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_configuration_a_finished_modification_leaves_is_what_the_next_one_reads() {
    let (router, _engine) = harness();
    // A 10 GiB volume of the default type, which provisions neither IOPS nor
    // configurable throughput.
    let id = create_volume(
        &router,
        json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 10 }),
    )
    .await;

    // Grown to 100 GiB and turned into a type that provisions both.
    let (status, payload) = modify(
        &router,
        &id,
        &json!({ "SizeInGB": 100, "VolumeType": "gp3" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the first modification: expected HTTP 200, got {status} with {payload}"
    );

    tokio::time::sleep(wsinst::state::VOLUME_MODIFICATION_SPAN + SLACK).await;

    // The new size is the CURRENT size now, so a smaller target is refused — which is
    // only true if the finished modification was applied.
    let (status, payload) = modify(&router, &id, &json!({ "SizeInGB": 50 })).await;
    assert_eq!(
        status, 400,
        "case shrinking past the applied size: expected HTTP 400, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "case shrinking past the applied size: expected ValidationException, got {payload}"
    );

    // And the new type is the current type: `Throughput` is configurable only for it,
    // so this call would have been refused against the type the volume started as. The
    // members it does not name are retained from the APPLIED configuration — the size
    // it was grown to, and the IOPS the type change defaulted.
    let (status, payload) = modify(&router, &id, &json!({ "Throughput": 1000 })).await;
    assert_eq!(
        status, 200,
        "case a member only the applied type takes: expected HTTP 200, got {status} with {payload}"
    );
    assert_eq!(
        (
            payload["TargetSize"].as_i64(),
            payload["TargetVolumeType"].as_str(),
            payload["TargetIops"].as_i64(),
            payload["TargetThroughput"].as_i64(),
        ),
        (Some(100), Some("gp3"), Some(3000), Some(1000)),
        "case a member only the applied type takes: expected the applied configuration to be \
         retained, got {payload}"
    );
}

/// One row of the refusal table: the volume to create, the modification to ask for,
/// and the members the refusal must name.
struct RefusalRow {
    case: &'static str,
    create: Value,
    modify: Value,
    members: Vec<&'static str>,
}

/// The modifications the refusal rule is asserted over: the call that asks for no
/// change at all, a size below the volume's own, each published bound of a size, an
/// IOPS and a throughput range (the value one past each end), and a performance member
/// the target type takes no value for — whether the type is the one the volume has or
/// the one the same call changes it to.
fn refusal_rows() -> Vec<RefusalRow> {
    let gp3 = json!({
        "AvailabilityZone": "us-east-1a",
        "VolumeType": "gp3",
        "SizeInGB": 125,
        "Iops": 3000,
        "Throughput": 200,
    });
    vec![
        RefusalRow {
            case: "no modifiable member at all",
            create: simple_create("us-east-1a"),
            modify: json!({}),
            members: vec!["SizeInGB", "VolumeType", "Iops", "Throughput"],
        },
        RefusalRow {
            case: "SizeInGB one below the volume's current size",
            create: json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 40 }),
            modify: json!({ "SizeInGB": 39 }),
            members: vec!["SizeInGB"],
        },
        RefusalRow {
            case: "SizeInGB one past the ceiling its own type publishes",
            create: json!({
                "AvailabilityZone": "us-east-1b",
                "VolumeType": "standard",
                "SizeInGB": 100,
            }),
            modify: json!({ "SizeInGB": 1025 }),
            members: vec!["SizeInGB"],
        },
        RefusalRow {
            case: "a type change the volume's size is below the floor of",
            create: json!({ "AvailabilityZone": "us-east-1c", "SizeInGB": 4 }),
            modify: json!({ "VolumeType": "st1" }),
            members: vec!["SizeInGB"],
        },
        RefusalRow {
            case: "Iops for a type that provisions none",
            create: json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 4 }),
            modify: json!({ "Iops": 3000 }),
            members: vec!["Iops"],
        },
        RefusalRow {
            case: "Iops for the TARGET type of a change, which provisions none",
            create: gp3.clone(),
            modify: json!({ "VolumeType": "gp2", "Iops": 3000 }),
            members: vec!["Iops"],
        },
        RefusalRow {
            case: "Iops one below the floor its type publishes",
            create: gp3.clone(),
            modify: json!({ "Iops": 2999 }),
            members: vec!["Iops"],
        },
        RefusalRow {
            case: "Iops one past the ceiling its type publishes",
            create: gp3.clone(),
            modify: json!({ "Iops": 80001 }),
            members: vec!["Iops"],
        },
        RefusalRow {
            case: "Throughput for a type whose throughput is not configurable",
            create: json!({
                "AvailabilityZone": "us-east-1b",
                "VolumeType": "io1",
                "SizeInGB": 4,
                "Iops": 100,
            }),
            modify: json!({ "Throughput": 125 }),
            members: vec!["Throughput"],
        },
        RefusalRow {
            case: "Throughput one below the floor its type publishes",
            create: gp3.clone(),
            modify: json!({ "Throughput": 124 }),
            members: vec!["Throughput"],
        },
        RefusalRow {
            case: "Throughput one past the ceiling its type publishes",
            create: gp3,
            modify: json!({ "Throughput": 2001 }),
            members: vec!["Throughput"],
        },
    ]
}

#[tokio::test]
async fn a_target_the_volume_type_does_not_accept_is_refused_and_records_nothing() {
    let (router, engine) = harness();

    for row in refusal_rows() {
        let case = row.case;
        let id = create_volume(&router, row.create.clone()).await;
        let (status, payload) = modify(&router, &id, &row.modify).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload}"
        );
        assert_eq!(
            payload["Reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: expected the field-validation reason, got {payload}"
        );
        let named: Vec<&str> = payload["FieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no FieldList in {payload}"))
            .iter()
            .map(|field| field["Name"].as_str().unwrap_or("<no Name>"))
            .collect();
        assert_eq!(
            named, row.members,
            "case {case:?}: expected the refusal to name {:?}, got {payload}",
            row.members
        );
        let message = payload["Message"]
            .as_str()
            .or_else(|| payload["message"].as_str())
            .unwrap_or_else(|| panic!("case {case:?}: no Message in {payload}"));
        for member in &row.members {
            assert!(
                message.contains(member),
                "case {case:?}: expected the wording to name {member:?}, got {message:?}"
            );
        }

        // A refused modify records nothing: the volume takes no modification, so it is
        // neither charged against the rolling limit nor reported as modifying.
        let stored = stored_volume(&engine, &id);
        assert!(
            stored.modifications.is_empty(),
            "case {case:?}: a refused modify must record nothing, got {:?}",
            stored.modifications
        );
    }
}

#[tokio::test]
async fn a_volume_id_that_names_no_volume_of_this_region_is_not_found() {
    let (router, _engine) = harness();
    // A populated store, so a refusal is the result of a lookup and not a blanket
    // answer.
    let stored = create_volume(&router, simple_create("us-east-1a")).await;
    let deleted = create_volume(&router, simple_create("us-east-1b")).await;
    ok(&router, "DeleteVolume", json!({ "VolumeId": &deleted })).await;

    // A volume of ANOTHER Region of the same account: its id was minted by this
    // service, and this Region's endpoint still holds nothing by that name.
    let foreign_region = "eu-west-3";
    assert!(
        wsinst::state::SUPPORTED_REGIONS.contains(&foreign_region),
        "case the foreign Region: {foreign_region:?} must be one the service serves, which are {:?}",
        wsinst::state::SUPPORTED_REGIONS
    );
    let (status, payload) = call_in_region(
        &router,
        foreign_region,
        "CreateVolume",
        json!({ "AvailabilityZone": "eu-west-3a", "SizeInGB": 4 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the foreign volume: expected HTTP 200, got {status} with {payload}"
    );
    let foreign = payload["VolumeId"]
        .as_str()
        .unwrap_or_else(|| panic!("case the foreign volume: no VolumeId in {payload}"))
        .to_string();

    let longest = format!("vol-{}", "z".repeat(63));
    let unknown = [
        ("the shortest legal id", "vol-a".to_string()),
        ("the longest legal id", longest),
        ("a mixed-case id", "vol-AbCdEf0123".to_string()),
        ("an id this service minted and deleted", deleted),
        ("an id of a volume another Region holds", foreign),
    ];

    for (case, id) in &unknown {
        assert_ne!(
            id, &stored,
            "case {case:?}: the test's own id must not be the stored one"
        );
        let (status, payload) = modify(&router, id, &json!({ "SizeInGB": 100 })).await;
        let expected_status = wsinst::state::RESOURCE_NOT_FOUND_STATUS;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({id:?}): expected HTTP {expected_status}, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?} ({id:?}): expected ResourceNotFoundException, got {payload}"
        );
        assert_eq!(
            payload["ResourceId"].as_str(),
            Some(id.as_str()),
            "case {case:?} ({id:?}): expected the error to name the id, got {payload}"
        );
        assert_eq!(
            payload["ResourceType"].as_str(),
            Some(wsinst::state::VOLUME_RESOURCE_TYPE),
            "case {case:?} ({id:?}): expected the volume resource type, got {payload}"
        );
        let message = payload["Message"]
            .as_str()
            .or_else(|| payload["message"].as_str());
        assert_eq!(
            message,
            Some(wsinst::state::VOLUME_NOT_FOUND_MESSAGE),
            "case {case:?} ({id:?}): expected the volume-not-found wording, got {payload}"
        );
    }

    // The volume this Region does hold is still modifiable: the refusals above
    // resolved ids, they did not blanket-refuse the operation.
    let (status, payload) = modify(&router, &stored, &json!({ "SizeInGB": 100 })).await;
    assert_eq!(
        status, 200,
        "case the stored volume: expected HTTP 200, got {status} with {payload}"
    );
}

/// The 429 a modify is refused with, asserting the shape's members and the wording
/// the service states for `message`.
async fn assert_rate_exceeded(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    body: &Value,
    expected_message: &str,
) {
    let (status, payload) = modify(router, id, body).await;
    assert_eq!(
        status, 429,
        "case {case:?}: expected HTTP 429, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "VolumeModificationRateExceededException",
        "case {case:?}: expected VolumeModificationRateExceededException, got {payload}"
    );
    assert_eq!(
        payload["ResourceId"].as_str(),
        Some(id),
        "case {case:?}: expected the error to name the volume, got {payload}"
    );
    assert_eq!(
        payload["ResourceType"].as_str(),
        Some(wsinst::state::VOLUME_RESOURCE_TYPE),
        "case {case:?}: expected the volume resource type, got {payload}"
    );
    let message = payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str());
    assert_eq!(
        message,
        Some(expected_message),
        "case {case:?}: expected the service's wording for this refusal, got {payload}"
    );
}

#[tokio::test]
async fn one_modification_runs_at_a_time_and_the_rolling_limit_bounds_them() {
    let (router, engine) = harness();
    let id = create_volume(
        &router,
        json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 1 }),
    )
    .await;

    // The modification the limit is counted in: each accepted one grows the volume, so
    // no two rows are the same request and none is refused for its own target.
    let accepted = wsinst::state::VOLUME_MODIFICATIONS_PER_WINDOW;
    assert!(
        accepted >= 2,
        "case the limit: the service must admit more than one modification per window, got {accepted}"
    );
    for n in 1..=accepted {
        let size = i64::try_from(n).expect("the count fits an i64") * 10;
        let (status, payload) = modify(&router, &id, &json!({ "SizeInGB": size })).await;
        assert_eq!(
            status, 200,
            "case modification {n} of {accepted}: expected HTTP 200, got {status} with {payload}"
        );

        // While it runs, a DIFFERENT modification of the same volume is refused: "You
        // can modify a volume only after any previous modification for that volume has
        // finished."
        assert_rate_exceeded(
            &router,
            "a second modification while the first runs",
            &id,
            &json!({ "SizeInGB": size + 5 }),
            wsinst::state::VOLUME_MODIFICATION_IN_PROGRESS_MESSAGE,
        )
        .await;

        tokio::time::sleep(wsinst::state::VOLUME_MODIFICATION_SPAN + SLACK).await;
    }

    // Past the limit, and with nothing running, the refusal is the other one the same
    // modelled error states.
    assert_rate_exceeded(
        &router,
        "one modification past the rolling limit",
        &id,
        &json!({ "SizeInGB": 1000 }),
        wsinst::state::VOLUME_MODIFICATION_LIMIT_MESSAGE,
    )
    .await;

    // Only the accepted modifications were recorded: a refused one is charged to
    // nothing, and the last accepted target is the configuration by now.
    let stored = stored_volume(&engine, &id);
    assert_eq!(
        stored.modifications.len(),
        accepted,
        "case the recorded modifications: expected {accepted}, got {:?}",
        stored.modifications
    );
    let last_size = i64::try_from(accepted).expect("the count fits an i64") * 10;
    assert_eq!(
        stored.size_in_gb.map(i64::from),
        Some(last_size),
        "case the applied size: expected the last accepted target {last_size}, got {stored:?}"
    );
}

#[tokio::test]
async fn an_idempotency_token_replays_one_modification_and_a_reuse_is_a_conflict() {
    let (router, engine) = harness();

    for token in ["tok-modify", "a token with spaces", &"t".repeat(64)] {
        let id = create_volume(
            &router,
            json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 4 }),
        )
        .await;
        let body = json!({ "SizeInGB": 40, "VolumeType": "gp3", "ClientToken": token });

        let (status, first) = modify(&router, &id, &body).await;
        assert_eq!(
            status, 200,
            "case token {token:?}: expected HTTP 200, got {status} with {first}"
        );
        // The retry replays the SAME target, rather than being refused for the
        // modification the first call started.
        let (status, replay) = modify(&router, &id, &body).await;
        assert_eq!(
            status, 200,
            "case token {token:?} replayed: expected HTTP 200, got {status} with {replay}"
        );
        assert_eq!(
            replay, first,
            "case token {token:?}: expected the same answer back, got {first} then {replay}"
        );
        let stored = stored_volume(&engine, &id);
        assert_eq!(
            stored.modifications.len(),
            1,
            "case token {token:?}: a retry must start no second modification, got {:?}",
            stored.modifications
        );

        // The same token for DIFFERENT parameters is a different request under a spent
        // token, and is refused — naming the volume the token did modify.
        let mut different = body.clone();
        different["SizeInGB"] = json!(80);
        let (status, payload) = modify(&router, &id, &different).await;
        assert_eq!(
            status, 409,
            "case token {token:?} reused: expected HTTP 409, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ConflictException",
            "case token {token:?} reused: expected ConflictException, got {payload}"
        );
        assert_eq!(
            payload["ResourceId"].as_str(),
            Some(id.as_str()),
            "case token {token:?} reused: expected the error to name the volume, got {payload}"
        );
        assert_eq!(
            payload["ResourceType"].as_str(),
            Some(wsinst::state::VOLUME_RESOURCE_TYPE),
            "case token {token:?} reused: expected the volume resource type, got {payload}"
        );
        let message = payload["Message"]
            .as_str()
            .or_else(|| payload["message"].as_str());
        assert_eq!(
            message,
            Some(wsinst::state::CLIENT_TOKEN_MISMATCH_MESSAGE),
            "case token {token:?} reused: expected the spent-token wording, got {payload}"
        );
        assert_eq!(
            stored_volume(&engine, &id).modifications.len(),
            1,
            "case token {token:?} reused: the refusal must record nothing"
        );
    }
}

#[tokio::test]
async fn modifications_live_in_the_engine_the_service_was_built_from() {
    // Two routers over two engines: a volume one of them minted is unknown to the
    // other, so nothing here is answered from the request.
    let (first, _first_engine) = harness();
    let (second, _second_engine) = harness();

    let id = create_volume(&first, simple_create("us-east-1a")).await;
    let (status, payload) = modify(&second, &id, &json!({ "SizeInGB": 100 })).await;
    assert_eq!(
        status,
        wsinst::state::RESOURCE_NOT_FOUND_STATUS,
        "case a cross-engine modify: expected the not-found status, got {status} with {payload}"
    );
    let (status, payload) = modify(&first, &id, &json!({ "SizeInGB": 100 })).await;
    assert_eq!(
        status, 200,
        "case the engine that minted it: expected HTTP 200, got {status} with {payload}"
    );
}
