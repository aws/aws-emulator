// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the volume slice: `CreateVolume` and `DeleteVolume`, driven
//! over the wire this service really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`).
//!
//! The rule under test: `DeleteVolume` resolves its `VolumeId` against the stored
//! volumes — it removes the record that id names and answers 200, and answers the
//! modelled `ResourceNotFoundException` for an id that satisfies the modelled
//! `VolumeId` shape but names no stored volume (including one this service itself
//! minted and has already deleted). The status and the resource-kind token of that
//! refusal are this service's own, asserted through `state::RESOURCE_NOT_FOUND_STATUS`
//! and `state::VOLUME_RESOURCE_TYPE` so the volume paths cannot drift from the
//! instance paths. Nothing here can pass against a canned answer: every id comes back
//! out of the store, and a delete only counts once the SECOND delete of the same id is
//! refused as not found.
//!
//! The wire helpers come from the sibling `workspace_instance_lifecycle` suite.

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{call, create, error_code, minimal_create, ok, router};

/// `VolumeId`'s modelled shape: `^vol-[0-9a-zA-Z]{1,63}$`
/// (`smithy_query(query="shape",shape="VolumeId")`).
fn is_modelled_volume_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("vol-") else {
        return false;
    };
    (1..=63).contains(&suffix.len()) && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The smallest `CreateVolume` body the service accepts in `zone`: a volume needs a
/// zone of the signed Region and a capacity source, and nothing else.
///
/// `pub(crate)` with [`create_volume`] below: the sibling `volume_attachment` suite
/// creates its volumes through the same two helpers rather than rebuilding them.
pub(crate) fn simple_create(zone: &str) -> Value {
    json!({ "AvailabilityZone": zone, "SizeInGB": 4 })
}

/// `CreateVolume`, returning the minted id.
pub(crate) async fn create_volume(router: &MultiServiceRouter, body: Value) -> String {
    let payload = ok(router, "CreateVolume", body).await;
    let id = payload
        .get("VolumeId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateVolume returned no VolumeId: {payload}"))
        .to_string();
    assert!(
        is_modelled_volume_id(&id),
        "case create: expected a modelled VolumeId, got {id:?}"
    );
    id
}

/// `DeleteVolume`, asserting the service's not-found answer and its resource members.
async fn assert_not_found(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, payload) = call(router, "DeleteVolume", json!({ "VolumeId": id })).await;
    // The status this service answers `ResourceNotFoundException` with, for whichever
    // resource was looked up — the same constant the instance paths answer with.
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
        Some(id),
        "case {case:?} ({id:?}): expected the error to name the id, got {payload}"
    );
    // The resource-kind token this service reports for a volume: upper case and
    // underscore separated, the form its recorded errors use.
    assert_eq!(
        payload["ResourceType"].as_str(),
        Some(wsinst::state::VOLUME_RESOURCE_TYPE),
        "case {case:?} ({id:?}): expected the volume resource type, got {payload}"
    );
    // The wording is the service's own fixed message for a missing volume, identical
    // for every id: the id itself travels in `ResourceId` above.
    let message = payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str());
    assert_eq!(
        message,
        Some(wsinst::state::VOLUME_NOT_FOUND_MESSAGE),
        "case {case:?} ({id:?}): expected the service's volume-not-found wording, got {payload}"
    );
}

/// One `CreateVolume` request body, named for the dimension it varies.
struct CreateRow {
    case: &'static str,
    body: Value,
}

/// A size and the IOPS `volume_type` needs, read off the service's own per-type
/// catalog (`wsinst::state::VOLUME_TYPE_SPECS`): the smallest volume of that type,
/// with provisioned IOPS only where the type takes them.
fn minimal_for(volume_type: &str) -> Value {
    let spec = wsinst::state::volume_type_spec(volume_type)
        .unwrap_or_else(|| panic!("case {volume_type:?}: the service publishes no rules for it"));
    let mut body = json!({
        "AvailabilityZone": "us-east-1a",
        "VolumeType": volume_type,
        "SizeInGB": spec.size_range().0,
    });
    if let Some((min, _)) = spec.iops {
        body["Iops"] = json!(min);
    }
    body
}

/// The create bodies the delete rule is asserted over: one row per dimension of the
/// input it generalises over — each Availability Zone of the signed Region, each
/// published `VolumeType` at its own minimum size, both ends of what `SizeInGB`
/// admits, the performance members, encryption, a snapshot source, tags, and an
/// idempotency token.
///
/// Every row is a configuration the backing EBS volume service would accept; the
/// sibling `volume_configuration_validation` suite asserts the refusals.
fn create_rows() -> Vec<CreateRow> {
    // `VolumeTypeEnum`'s full value list, from the model
    // (`smithy_query(query="shape",shape="VolumeTypeEnum")`); the `VOLUME_TYPE_ENUM_VALUES`
    // constant the crate generates from it is the same list.
    let volume_types = wsinst::ops::types::VOLUME_TYPE_ENUM_VALUES;
    assert!(
        volume_types.contains(&"st1"),
        "case catalog: the generated VolumeTypeEnum list must carry the modelled values, got {volume_types:?}"
    );

    let mut rows = vec![
        CreateRow {
            case: "the smallest volume of the default type",
            body: json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 1 }),
        },
        CreateRow {
            case: "SizeInGB at the top of the modelled range",
            body: json!({ "AvailabilityZone": "us-east-1b", "SizeInGB": 16384 }),
        },
        CreateRow {
            case: "gp3 with Iops and Throughput at the bottom of their ranges",
            body: json!({
                "AvailabilityZone": "us-east-1c",
                "VolumeType": "gp3",
                "SizeInGB": 125,
                "Iops": 3000,
                "Throughput": 125,
            }),
        },
        CreateRow {
            case: "an encrypted volume with a KMS key",
            body: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 20,
                "Encrypted": true,
                "KmsKeyId": "arn:aws:kms:us-east-1:123456789012:key/abcd",
            }),
        },
        CreateRow {
            case: "restored from a snapshot, which carries the size",
            body: json!({ "AvailabilityZone": "us-east-1b", "SnapshotId": "snap-0123456789abcdef" }),
        },
        CreateRow {
            case: "tagged for the volume resource type",
            body: json!({
                "AvailabilityZone": "us-east-1c",
                "SizeInGB": 4,
                "TagSpecifications": [{
                    "ResourceType": "volume",
                    "Tags": [{ "Key": "env", "Value": "test" }],
                }],
            }),
        },
        CreateRow {
            case: "carrying an idempotency token",
            body: json!({
                "AvailabilityZone": "us-east-1c",
                "SizeInGB": 8,
                "ClientToken": "tok-row-volume",
            }),
        },
    ];

    for volume_type in volume_types {
        rows.push(CreateRow {
            case: "a published VolumeType at its minimum size",
            body: minimal_for(volume_type),
        });
    }

    rows
}

#[tokio::test]
async fn delete_removes_the_volume_the_id_names_and_a_second_delete_is_not_found() {
    let router = router();

    let rows = create_rows();
    let mut created = Vec::new();
    for row in &rows {
        let id = create_volume(&router, row.body.clone()).await;
        assert!(
            !created.contains(&id),
            "case {:?}: expected a fresh VolumeId, got the already-minted {id:?}",
            row.case
        );
        created.push(id);
    }

    // Every id resolves to its own record: deleting one answers 200 and an empty
    // response body, and leaves every other volume deletable.
    for (row, id) in rows.iter().zip(&created) {
        let payload = ok(&router, "DeleteVolume", json!({ "VolumeId": id })).await;
        assert!(
            payload.is_null() || payload.as_object().is_some_and(|body| body.is_empty()),
            "case {:?} ({id:?}): DeleteVolumeResponse models no members, got {payload}",
            row.case
        );
        assert_not_found(&router, row.case, id).await;
    }
}

#[tokio::test]
async fn an_unknown_but_well_formed_volume_id_is_not_found() {
    let router = router();
    // A populated store, so the refusal is the result of a lookup and not a blanket
    // answer.
    let stored = create_volume(&router, simple_create("us-east-1a")).await;

    // Well-formed per `^vol-[0-9a-zA-Z]{1,63}$`, stored by nobody: the shortest legal
    // id, the longest legal id, a mixed-case one and an all-digit one.
    let longest = format!("vol-{}", "z".repeat(63));
    let unknown = [
        ("the shortest legal id", "vol-a".to_string()),
        ("the longest legal id", longest),
        ("a mixed-case id", "vol-AbCdEf0123".to_string()),
        ("an all-digit id", "vol-98765432109876543".to_string()),
    ];

    for (case, id) in &unknown {
        assert!(
            is_modelled_volume_id(id),
            "case {case:?}: the test's own id must satisfy the model, got {id:?}"
        );
        assert_ne!(
            id, &stored,
            "case {case:?}: the test's unknown id must not be the stored one"
        );
        assert_not_found(&router, case, id).await;
    }

    // The stored volume is untouched by those refusals: it still deletes with a 200.
    ok(&router, "DeleteVolume", json!({ "VolumeId": &stored })).await;
}

#[tokio::test]
async fn a_volume_id_outside_the_modelled_shape_is_refused() {
    let router = router();

    // Each row breaks exactly one part of `^vol-[0-9a-zA-Z]{1,63}$`; an absent
    // required member is the same 400.
    let too_long = format!("vol-{}", "a".repeat(64));
    let malformed: Vec<(&str, &str)> = vec![
        ("empty", ""),
        ("prefix only", "vol-"),
        ("no prefix", "0123456789"),
        ("the wrong prefix", "volume-0123456789"),
        ("the wrong prefix case", "VOL-0123456789"),
        ("a character outside the suffix class", "vol-abc_def"),
        ("a suffix one past the maximum", &too_long),
        ("a leading space", " vol-0123456789"),
        ("another resource's id", "wsinst-0123456789"),
    ];

    for (case, id) in malformed {
        let (status, payload) = call(&router, "DeleteVolume", json!({ "VolumeId": id })).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({id:?}): expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?} ({id:?}): expected ValidationException, got {payload}"
        );
    }

    for (case, operation, body) in [
        ("DeleteVolume without VolumeId", "DeleteVolume", json!({})),
        (
            "CreateVolume without AvailabilityZone",
            "CreateVolume",
            json!({ "SizeInGB": 8 }),
        ),
    ] {
        let (status, payload) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload}"
        );
    }
}

#[tokio::test]
async fn deleting_one_volume_touches_no_other_resource() {
    let router = router();
    let instance = create(&router, minimal_create()).await;
    let volumes: Vec<String> = {
        let mut ids = Vec::new();
        for zone in ["us-east-1a", "us-east-1b", "us-east-1c"] {
            ids.push(create_volume(&router, simple_create(zone)).await);
        }
        ids
    };

    ok(&router, "DeleteVolume", json!({ "VolumeId": &volumes[1] })).await;
    assert_not_found(&router, "the deleted volume", &volumes[1]).await;

    // The siblings survive, and so does the WorkSpace Instance: a volume delete
    // resolves one id in one map.
    for id in [&volumes[0], &volumes[2]] {
        ok(&router, "DeleteVolume", json!({ "VolumeId": id })).await;
    }
    let got = ok(
        &router,
        "GetWorkspaceInstance",
        json!({ "WorkspaceInstanceId": &instance }),
    )
    .await;
    assert_eq!(
        got["WorkspaceInstanceId"].as_str(),
        Some(instance.as_str()),
        "case instance after volume deletes: expected {instance:?} to survive, got {got}"
    );
}

#[tokio::test]
async fn a_spent_idempotency_token_is_refused_and_retired_by_delete() {
    let router = router();

    for token in ["tok-1", "a token with spaces", &"t".repeat(64)] {
        let mut body = simple_create("us-east-1a");
        body["ClientToken"] = json!(token);
        let first = create_volume(&router, body.clone()).await;
        // The token is spent by that create, so sending the identical request again is
        // refused with the conflict naming the volume it did create.
        let (status, payload) = call(&router, "CreateVolume", body.clone()).await;
        assert_eq!(
            status, 409,
            "case token {token:?}: expected HTTP 409 for a spent token, got {status} with {payload}"
        );
        assert_eq!(
            payload["ResourceId"].as_str(),
            Some(first.as_str()),
            "case token {token:?}: expected the conflict to name {first:?}, got {payload}"
        );

        // ONE volume was created, so deleting it once is enough to make the id unknown.
        ok(&router, "DeleteVolume", json!({ "VolumeId": &first })).await;
        assert_not_found(&router, "a volume created once under a token, deleted", &first).await;

        // The token went with the record, so a create carrying it is a new request again
        // rather than a conflict with an id that no longer exists.
        let after_delete = create_volume(&router, body).await;
        assert_ne!(
            after_delete, first,
            "case token {token:?}: expected a new volume after the delete, got the deleted {first:?}"
        );
        ok(&router, "DeleteVolume", json!({ "VolumeId": &after_delete })).await;
    }
}

#[tokio::test]
async fn the_volume_quota_is_bounded_and_released_by_delete() {
    let router = router();
    let quota = wsinst::state::MAX_VOLUMES;

    let mut created = Vec::new();
    for n in 1..=quota {
        let (status, payload) = call(&router, "CreateVolume", simple_create("us-east-1a")).await;
        assert_eq!(
            status, 200,
            "case volume {n} of the quota {quota}: expected HTTP 200, got {status} with {payload}"
        );
        created.push(
            payload["VolumeId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    let (status, payload) = call(&router, "CreateVolume", simple_create("us-east-1a")).await;
    assert_eq!(
        status, 402,
        "case one past the quota: expected HTTP 402, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ServiceQuotaExceededException",
        "case one past the quota: expected ServiceQuotaExceededException, got {payload}"
    );
    assert_eq!(
        payload["ResourceType"].as_str(),
        Some(wsinst::state::VOLUME_RESOURCE_TYPE),
        "case one past the quota: expected the volume resource type, got {payload}"
    );

    // DeleteVolume releases the capacity it was holding, which is only true if the
    // record really left the store.
    ok(&router, "DeleteVolume", json!({ "VolumeId": &created[0] })).await;
    let (status, payload) = call(&router, "CreateVolume", simple_create("us-east-1a")).await;
    assert_eq!(
        status, 200,
        "case create after a delete freed capacity: expected HTTP 200, got {status} with {payload}"
    );
}

#[tokio::test]
async fn volumes_live_in_the_engine_the_service_was_built_from() {
    // Two routers over two engines: a volume one of them minted is unknown to the
    // other, so nothing here is answered from the request.
    let first = router();
    let second = router();

    let id = create_volume(&first, simple_create("us-east-1a")).await;
    assert_not_found(&second, "a cross-engine delete", &id).await;
    ok(&first, "DeleteVolume", json!({ "VolumeId": &id })).await;
}
