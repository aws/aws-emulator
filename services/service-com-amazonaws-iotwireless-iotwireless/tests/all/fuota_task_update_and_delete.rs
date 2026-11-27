// SPDX-License-Identifier: CC0-1.0

//! The FUOTA task's EFFECTIVE configuration, its updates and its deletion, end to end
//! over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * a created task's transfer plan is COMPLETE: for each optional fragmentation
//!   member the create carried, the reader reports that value, and for each one it
//!   omitted the reader reports the default the service applied;
//! * a read of a task reports the members `GetFuotaTaskResponse` declares and NOTHING
//!   else, whichever optional members the create carried;
//! * `UpdateFuotaTask` applies every member the patch carries to the stored task and
//!   leaves every member it omits exactly as it stood, so the reader reports the
//!   updated configuration;
//! * `DeleteFuotaTask` removes the task, so every later call that addresses it is the
//!   modelled 404 — status, error type, message, `ResourceId` and `ResourceType` —
//!   while the tasks it did not name keep resolving and no device reports membership of
//!   a task the store no longer holds.
//!
//! The wire harness is the one the sibling suites already have: `tests/all/*.rs` are
//! modules of ONE test target, so `request_signed_for`, `call`, `error_code` and
//! `router` are shared from there rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The fragmentation defaults the service publishes, so this suite asserts what the
/// implementation declares rather than numbers copied by hand.
use service_com_amazonaws_iotwireless_iotwireless::state::{
    DEFAULT_FUOTA_FRAGMENT_INTERVAL_MS, DEFAULT_FUOTA_FRAGMENT_SIZE_BYTES,
    DEFAULT_FUOTA_REDUNDANCY_PERCENT, FUOTA_TASK_RESOURCE_TYPE,
};

use super::destination_and_wireless_device_lifecycle::{
    call, error_code, request_signed_for, router, REGION,
};

/// The two required members of a FUOTA task, reused by every case.
const IMAGE: &str = "s3://fuota-images/update-and-delete.bin";
const ROLE: &str = "arn:aws:iam::000000000000:role/iotwireless-fuota";

/// Every member `GetFuotaTask`'s output shape declares, and nothing else.
/// Source: `smithy_query(query="operation", operation="GetFuotaTask")`.
const GET_FUOTA_TASK_OUTPUT_MEMBERS: &[&str] = &[
    "Arn",
    "CreatedAt",
    "Description",
    "Descriptor",
    "FirmwareUpdateImage",
    "FirmwareUpdateRole",
    "FragmentIntervalMS",
    "FragmentSizeBytes",
    "Id",
    "LoRaWAN",
    "Name",
    "RedundancyPercent",
    "Status",
];

/// Every member the nested `LoRaWAN` structure of a task read declares.
/// Source: `smithy_query(query="shape", shape="LoRaWANFuotaTaskGetInfo")`.
const LORAWAN_GET_INFO_MEMBERS: &[&str] = &["RfRegion", "StartTime"];

/// The members a STORED task always has, so a read of one reports every one of them
/// whatever the create carried: its identity, its two required members, its status, its
/// creation instant and the three members of its transfer plan.
const ALWAYS_REPORTED_MEMBERS: &[&str] = &[
    "Arn",
    "CreatedAt",
    "FirmwareUpdateImage",
    "FirmwareUpdateRole",
    "FragmentIntervalMS",
    "FragmentSizeBytes",
    "Id",
    "RedundancyPercent",
    "Status",
];

/// The three optional fragmentation members, each beside the default the service
/// applies when a create names none.
/// Source: `smithy_query(query="operation", operation="CreateFuotaTask")`.
fn fragmentation_defaults() -> [(&'static str, i64); 3] {
    [
        (
            "RedundancyPercent",
            i64::from(DEFAULT_FUOTA_REDUNDANCY_PERCENT),
        ),
        (
            "FragmentSizeBytes",
            i64::from(DEFAULT_FUOTA_FRAGMENT_SIZE_BYTES),
        ),
        (
            "FragmentIntervalMS",
            i64::from(DEFAULT_FUOTA_FRAGMENT_INTERVAL_MS),
        ),
    ]
}

/// The defaults themselves: each one inside its member's PUBLISHED range, and each the
/// value the service applies — half the fragments redundant, fragments of 50 bytes, and
/// one second between them (the granularity `FragmentIntervalMS`'s own model
/// documentation gives it: "rounded to the nearest second").
/// Source of the ranges: `smithy_query(query="operation", operation="CreateFuotaTask")`.
#[test]
fn every_published_fragmentation_default_is_inside_its_members_range() {
    // (member, the default the service publishes, its published range, the value)
    for (member, published, range, expected) in [
        (
            "RedundancyPercent",
            DEFAULT_FUOTA_REDUNDANCY_PERCENT,
            (0, 100),
            50,
        ),
        (
            "FragmentSizeBytes",
            DEFAULT_FUOTA_FRAGMENT_SIZE_BYTES,
            (1, i32::MAX),
            50,
        ),
        (
            "FragmentIntervalMS",
            DEFAULT_FUOTA_FRAGMENT_INTERVAL_MS,
            (1, i32::MAX),
            1_000,
        ),
    ] {
        let (min, max) = range;
        assert!(
            (min..=max).contains(&published),
            "case {member:?}: the default {published} is outside the published range \
             {min}..={max}"
        );
        assert_eq!(
            published, expected,
            "case {member:?}: expected the service default {expected}, got {published}"
        );
    }
}

async fn create_task(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/fuota-tasks", Some(body)).await
}

async fn get_task(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(router, Method::GET, &format!("/fuota-tasks/{id}"), None).await
}

async fn update_task(
    router: &MultiServiceRouter,
    id: &str,
    body: Value,
) -> (StatusCode, Value) {
    call(
        router,
        Method::PATCH,
        &format!("/fuota-tasks/{id}"),
        Some(body),
    )
    .await
}

async fn delete_task(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(router, Method::DELETE, &format!("/fuota-tasks/{id}"), None).await
}

/// One answer, including the `x-amzn-errortype` header restJson1 resolves an error
/// shape from — which `call` does not carry.
async fn answer(
    router: &MultiServiceRouter,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, String, Value) {
    let response = router
        .dispatch(request_signed_for(Some(REGION), method, uri, body))
        .await;
    let status = response.status();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, error_type, parsed)
}

/// Create one task from `body` and return its `Id`.
async fn seed_task(case: &str, router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = create_task(router, body).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "case {case:?}: expected 201 from the create, got {status} {created}"
    );
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Id in {created}"))
        .to_string()
}

/// The 404 body a FUOTA task addressed by an `Id` the store does not hold is refused
/// with: the sentence this service words for the class, with the resource kind and the
/// requested id interpolated, and both optional members carrying those two values.
fn task_refusal(resource_id: &str) -> Value {
    json!({
        "Message": format!(
            "Cannot complete the step because resource is not found, with resource type \
             {FUOTA_TASK_RESOURCE_TYPE}, resource Id {resource_id}"
        ),
        "ResourceId": resource_id,
        "ResourceType": FUOTA_TASK_RESOURCE_TYPE,
    })
}

/// Assert that `id` is refused as a missing FUOTA task: the modelled status, the error
/// type in the header restJson1 resolves the shape from, and the whole body.
async fn assert_task_not_found(
    case: &str,
    router: &MultiServiceRouter,
    method: Method,
    id: &str,
    body: Option<Value>,
) {
    let (status, error_type, answered) =
        answer(router, method, &format!("/fuota-tasks/{id}"), body).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "case {case:?}: expected 404 for {id:?}, got {status} {answered}"
    );
    assert_eq!(
        error_type, "ResourceNotFoundException",
        "case {case:?}: expected the modelled error shape in x-amzn-errortype, got \
         {error_type:?}"
    );
    assert_eq!(
        error_code(&answered),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answered}"
    );
    for member in ["Message", "ResourceId", "ResourceType"] {
        assert_eq!(
            answered.get(member),
            task_refusal(id).get(member),
            "case {case:?}: expected {member} = {:?}, got {:?}",
            task_refusal(id).get(member),
            answered.get(member)
        );
    }
}

/// A created task reports a COMPLETE transfer plan: each fragmentation member the
/// create pinned, and the service default for each one it left out. The cases span
/// every presence combination of the three members, at the bottom and the top of each
/// published range.
#[tokio::test]
async fn a_created_task_reports_its_effective_fragmentation_plan() {
    let router = router();

    // (case, the fragmentation members the create carries)
    let cases: Vec<(&str, Value)> = vec![
        ("none pinned", json!({})),
        ("redundancy only", json!({ "RedundancyPercent": 0 })),
        ("fragment size only", json!({ "FragmentSizeBytes": 1 })),
        ("interval only", json!({ "FragmentIntervalMS": 1 })),
        (
            "redundancy and size",
            json!({ "RedundancyPercent": 100, "FragmentSizeBytes": 4096 }),
        ),
        (
            "size and interval",
            json!({ "FragmentSizeBytes": 237, "FragmentIntervalMS": 60_000 }),
        ),
        (
            "all three pinned",
            json!({
                "RedundancyPercent": 37,
                "FragmentSizeBytes": 128,
                "FragmentIntervalMS": 2_500,
            }),
        ),
    ];

    for (case, pinned) in cases {
        let mut body = json!({ "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE });
        for (member, value) in pinned.as_object().expect("an object") {
            body[member] = value.clone();
        }

        let id = seed_task(case, &router, body).await;
        let (status, fetched) = get_task(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from the read, got {status} {fetched}"
        );

        for (member, default) in fragmentation_defaults() {
            let expected = pinned.get(member).cloned().unwrap_or(json!(default));
            assert_eq!(
                fetched.get(member),
                Some(&expected),
                "case {case:?}: expected {member} = {expected}, got {:?}",
                fetched.get(member)
            );
        }
    }
}

/// A read of a task reports the members its OUTPUT SHAPE declares and nothing else: no
/// member outside [`GET_FUOTA_TASK_OUTPUT_MEMBERS`] reaches the wire, every member a
/// stored task always has is there, and the nested `LoRaWAN` structure is held to its
/// own declared members. The cases span which optional members the create carried, so
/// the surface is asserted on a task that pinned none, on several that pinned one each
/// and on one that pinned them all.
#[tokio::test]
async fn a_read_task_reports_only_the_members_its_output_shape_models() {
    let router = router();

    // (case, the optional members the create carries beside the two required ones)
    let cases: Vec<(&str, Value)> = vec![
        ("required only", json!({})),
        ("named", json!({ "Name": "FuotaTask-surface" })),
        ("described", json!({ "Description": "the whole reported surface" })),
        ("lorawan region", json!({ "LoRaWAN": { "RfRegion": "US915" } })),
        ("descriptor", json!({ "Descriptor": "c3VyZmFjZQ==" })),
        (
            "every optional member",
            json!({
                "Name": "FuotaTask-surface-full",
                "Description": "every optional member at once",
                "LoRaWAN": { "RfRegion": "EU868" },
                "RedundancyPercent": 25,
                "FragmentSizeBytes": 64,
                "FragmentIntervalMS": 1_500,
                "Descriptor": "c3VyZmFjZS1mdWxs",
                "Tags": [{ "Key": "stage", "Value": "surface" }],
            }),
        ),
    ];

    for (case, optional) in cases {
        let mut body = json!({ "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE });
        for (member, value) in optional.as_object().expect("an object") {
            body[member] = value.clone();
        }

        let id = seed_task(case, &router, body).await;
        let (status, fetched) = get_task(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from the read, got {status} {fetched}"
        );

        let reported = fetched
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object, got {fetched}"));
        for member in reported.keys() {
            assert!(
                GET_FUOTA_TASK_OUTPUT_MEMBERS.contains(&member.as_str()),
                "case {case:?}: {member:?} is not one of the operation's modelled output \
                 members {GET_FUOTA_TASK_OUTPUT_MEMBERS:?}, got {fetched}"
            );
        }
        for member in ALWAYS_REPORTED_MEMBERS {
            assert!(
                reported.contains_key(*member),
                "case {case:?}: expected {member} in every read of a stored task, got \
                 {fetched}"
            );
        }
        if let Some(lorawan) = reported.get("LoRaWAN").and_then(Value::as_object) {
            for member in lorawan.keys() {
                assert!(
                    LORAWAN_GET_INFO_MEMBERS.contains(&member.as_str()),
                    "case {case:?}: LoRaWAN.{member} is not one of the nested structure's \
                     modelled members {LORAWAN_GET_INFO_MEMBERS:?}, got {fetched}"
                );
            }
        }
    }
}

/// Every member a patch carries replaces the stored one and every member it omits is
/// left as it stood, so the reader reports the updated configuration. The cases span
/// each updatable member on its own, several together, and the empty patch.
#[tokio::test]
async fn an_update_applies_its_members_and_leaves_the_others_standing() {
    let router = router();

    // The members `UpdateFuotaTask` takes beside `Id`.
    // Source: `smithy_query(query="operation", operation="UpdateFuotaTask")`.
    let cases: Vec<(&str, Value)> = vec![
        ("empty patch", json!({})),
        ("rename", json!({ "Name": "FuotaTask-renamed" })),
        ("name emptied", json!({ "Name": "" })),
        ("description", json!({ "Description": "second revision" })),
        (
            "lorawan region",
            json!({ "LoRaWAN": { "RfRegion": "AS923-2" } }),
        ),
        (
            "image and role",
            json!({
                "FirmwareUpdateImage": "s3://fuota-images/firmware-v2.bin",
                "FirmwareUpdateRole": "arn:aws:iam::000000000000:role/iotwireless-fuota-v2",
            }),
        ),
        ("redundancy at min", json!({ "RedundancyPercent": 0 })),
        ("redundancy at max", json!({ "RedundancyPercent": 100 })),
        ("fragment size at min", json!({ "FragmentSizeBytes": 1 })),
        ("interval at min", json!({ "FragmentIntervalMS": 1 })),
        ("descriptor", json!({ "Descriptor": "dmVyc2lvbjI=" })),
        (
            "every member at once",
            json!({
                "Name": "FuotaTask-everything",
                "Description": "patched whole",
                "LoRaWAN": { "RfRegion": "US915" },
                "FirmwareUpdateImage": "s3://fuota-images/firmware-v3.bin",
                "FirmwareUpdateRole": "arn:aws:iam::000000000000:role/iotwireless-fuota-v3",
                "RedundancyPercent": 11,
                "FragmentSizeBytes": 222,
                "FragmentIntervalMS": 3_333,
                "Descriptor": "dmVyc2lvbjM=",
            }),
        ),
    ];

    // The task every case starts from, so what the patch must LEAVE ALONE is known.
    let seed = json!({
        "Name": "FuotaTask-before",
        "Description": "first revision",
        "LoRaWAN": { "RfRegion": "EU868" },
        "FirmwareUpdateImage": IMAGE,
        "FirmwareUpdateRole": ROLE,
        "RedundancyPercent": 44,
        "FragmentSizeBytes": 77,
        "FragmentIntervalMS": 5_000,
        "Descriptor": "dmVyc2lvbjE=",
    });

    for (case, patch) in cases {
        let id = seed_task(case, &router, seed.clone()).await;

        let (status, answered) = update_task(&router, &id, patch.clone()).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from the patch, got {status} {answered}"
        );

        let (status, fetched) = get_task(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from the read, got {status} {fetched}"
        );

        for member in [
            "Name",
            "Description",
            "FirmwareUpdateImage",
            "FirmwareUpdateRole",
            "RedundancyPercent",
            "FragmentSizeBytes",
            "FragmentIntervalMS",
            "Descriptor",
        ] {
            let expected = patch.get(member).or_else(|| seed.get(member));
            assert_eq!(
                fetched.get(member),
                expected,
                "case {case:?}: expected {member} = {expected:?}, got {:?}",
                fetched.get(member)
            );
        }
        let expected_region = patch["LoRaWAN"]
            .get("RfRegion")
            .or_else(|| seed["LoRaWAN"].get("RfRegion"));
        assert_eq!(
            fetched["LoRaWAN"].get("RfRegion"),
            expected_region,
            "case {case:?}: expected LoRaWAN.RfRegion = {expected_region:?}, got {:?}",
            fetched["LoRaWAN"].get("RfRegion")
        );
        // The identity is not patchable, so it stands.
        assert_eq!(
            fetched["Id"],
            json!(id),
            "case {case:?}: a patch must not move the task's Id, got {:?}",
            fetched["Id"]
        );
    }
}

/// A patch that addresses an `Id` the store does not hold is the modelled 404 and
/// writes nothing — whatever the id looks like.
#[tokio::test]
async fn an_update_of_an_unknown_task_is_not_found() {
    let router = router();
    let present = seed_task(
        "present",
        &router,
        json!({
            "Name": "FuotaTask-present",
            "FirmwareUpdateImage": IMAGE,
            "FirmwareUpdateRole": ROLE,
        }),
    )
    .await;

    for id in [
        "00000000-0000-4000-8000-000000000000",
        "not-a-uuid",
        "A",
        &present.to_uppercase(),
    ] {
        assert_task_not_found(
            id,
            &router,
            Method::PATCH,
            id,
            Some(json!({ "Name": "FuotaTask-ghost" })),
        )
        .await;
    }

    let (status, fetched) = get_task(&router, &present).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the stored task must still resolve after the refused patches, got {status} {fetched}"
    );
    assert_eq!(
        fetched["Name"],
        json!("FuotaTask-present"),
        "a refused patch must change nothing, got {:?}",
        fetched["Name"]
    );
}

/// A deleted task is gone: the read, a second delete and a patch are all the modelled
/// 404, the tasks the delete did not name keep resolving, and the id can be minted
/// again by a later create.
#[tokio::test]
async fn a_deleted_task_stops_resolving_while_its_siblings_stand() {
    let router = router();

    // Several tasks, so what the delete must LEAVE ALONE is observable. The cases vary
    // in which member set the task was created with.
    let cases: Vec<(&str, Value)> = vec![
        (
            "required only",
            json!({ "FirmwareUpdateImage": IMAGE, "FirmwareUpdateRole": ROLE }),
        ),
        (
            "named and fragmented",
            json!({
                "Name": "FuotaTask-delete-2",
                "LoRaWAN": { "RfRegion": "US915" },
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
                "RedundancyPercent": 10,
                "FragmentSizeBytes": 20,
                "FragmentIntervalMS": 30,
            }),
        ),
        (
            "tagged with a token",
            json!({
                "Name": "FuotaTask-delete-3",
                "ClientRequestToken": "delete-token-3",
                "FirmwareUpdateImage": IMAGE,
                "FirmwareUpdateRole": ROLE,
                "Tags": [{ "Key": "stage", "Value": "test" }],
            }),
        ),
    ];

    let mut ids = Vec::new();
    for (case, body) in &cases {
        ids.push((*case, seed_task(case, &router, body.clone()).await));
    }

    for (index, (case, id)) in ids.iter().enumerate() {
        let (status, answered) = delete_task(&router, id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from the delete, got {status} {answered}"
        );

        // Everything that addresses the deleted task is now the modelled refusal.
        assert_task_not_found(case, &router, Method::GET, id, None).await;
        assert_task_not_found(case, &router, Method::DELETE, id, None).await;
        assert_task_not_found(
            case,
            &router,
            Method::PATCH,
            id,
            Some(json!({ "Name": "FuotaTask-after-delete" })),
        )
        .await;

        // Every task the delete did not name still reads back.
        for (sibling, sibling_id) in ids.iter().skip(index + 1) {
            let (status, fetched) = get_task(&router, sibling_id).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: sibling {sibling:?} must still resolve, got {status} {fetched}"
            );
        }
    }

    // A token whose task was deleted replays nothing: the create mints a NEW task.
    let (status, created) = create_task(
        &router,
        json!({
            "Name": "FuotaTask-after-delete-3",
            "ClientRequestToken": "delete-token-3",
            "FirmwareUpdateImage": IMAGE,
            "FirmwareUpdateRole": ROLE,
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a replayed token whose task was deleted must mint a new one, got {status} {created}"
    );
    let reminted = created["Id"].as_str().unwrap_or_default();
    assert!(
        !ids.iter().any(|(_, id)| id == reminted),
        "the new task must carry a fresh Id, got {reminted:?}"
    );
}

/// Deleting a task releases the membership of every device that belonged to it, so no
/// device reports membership of a task the store no longer holds while the devices
/// themselves stay registered.
#[tokio::test]
async fn deleting_a_task_releases_its_device_memberships() {
    let router = router();

    let (status, destination) = call(
        &router,
        Method::POST,
        "/destinations",
        Some(json!({
            "Name": "FuotaDeleteDest",
            "ExpressionType": "RuleName",
            "Expression": "rule-for-fuota-delete",
            "RoleArn": "arn:aws:iam::000000000000:role/iotwireless-destination",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding the destination: expected 201, got {status} {destination}"
    );

    let kept = seed_task(
        "kept",
        &router,
        json!({
            "Name": "FuotaTask-kept",
            "FirmwareUpdateImage": IMAGE,
            "FirmwareUpdateRole": ROLE,
        }),
    )
    .await;
    let doomed = seed_task(
        "doomed",
        &router,
        json!({
            "Name": "FuotaTask-doomed",
            "FirmwareUpdateImage": IMAGE,
            "FirmwareUpdateRole": ROLE,
        }),
    )
    .await;

    // One device per task, of both modelled wireless types, so the release is asserted
    // over more than one member.
    let mut members: Vec<(&str, String)> = Vec::new();
    for (case, task, body) in [
        (
            "lorawan-doomed",
            &doomed,
            json!({
                "Type": "LoRaWAN",
                "Name": "fuota-delete-lorawan",
                "DestinationName": "FuotaDeleteDest",
                "LoRaWAN": { "DevEui": "0000000000000d01" },
            }),
        ),
        (
            "sidewalk-doomed",
            &doomed,
            json!({
                "Type": "Sidewalk",
                "Name": "fuota-delete-sidewalk",
                "DestinationName": "FuotaDeleteDest",
                "Sidewalk": { "DeviceProfileId": "dp-sidewalk" },
            }),
        ),
        (
            "lorawan-kept",
            &kept,
            json!({
                "Type": "LoRaWAN",
                "Name": "fuota-keep-lorawan",
                "DestinationName": "FuotaDeleteDest",
                "LoRaWAN": { "DevEui": "0000000000000d02" },
            }),
        ),
    ] {
        let (status, created) = call(&router, Method::POST, "/wireless-devices", Some(body)).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case:?}: expected 201 from the device create, got {status} {created}"
        );
        let device_id = created["Id"].as_str().expect("an Id").to_string();
        let (status, answered) = call(
            &router,
            Method::PUT,
            &format!("/fuota-tasks/{task}/wireless-device"),
            Some(json!({ "WirelessDeviceId": device_id })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from the associate, got {status} {answered}"
        );
        members.push((case, device_id));
    }

    let (status, answered) = delete_task(&router, &doomed).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "expected 204 from the delete, got {status} {answered}"
    );

    for (task, expected) in [(&doomed, 0usize), (&kept, 1usize)] {
        let (status, listed) = call(
            &router,
            Method::GET,
            &format!("/wireless-devices?fuotaTaskId={task}"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case fuotaTaskId={task}: expected 200, got {status} {listed}"
        );
        let served = listed["WirelessDeviceList"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            served, expected,
            "case fuotaTaskId={task}: expected {expected} device(s), got {served} in {listed}"
        );
    }

    // The devices themselves are untouched: only the membership was released.
    for (case, device_id) in &members {
        let (status, fetched) = call(
            &router,
            Method::GET,
            &format!("/wireless-devices/{device_id}?identifierType=WirelessDeviceId"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: the device must still resolve, got {status} {fetched}"
        );
    }
}
