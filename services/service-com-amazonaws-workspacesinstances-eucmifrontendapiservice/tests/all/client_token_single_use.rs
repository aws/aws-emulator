// SPDX-License-Identifier: CC0-1.0

//! A `ClientToken` of this service's `@idempotent` creates is SINGLE-USE, driven over the
//! wire this service really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`).
//!
//! RULE under test: the create that carries a `ClientToken` SPENDS it, so every later
//! create carrying that token is refused with the modelled `ConflictException` (HTTP 409,
//! `smithy_query(query="shape",shape="ConflictException")`) naming the resource the token
//! did create, and creates nothing — whether the later request's parameters are the ones
//! the token was spent on or different ones. The token guards against a duplicate
//! creation; it is not a key the first call's response is replayed from.
//!
//! The rows span what the rule generalises over: both operations the model gives a
//! `ClientToken` `@idempotencyToken` (`CreateWorkspaceInstance` and `CreateVolume`), the
//! token forms the modelled `ClientToken` admits (`len 1..64`, `^[\x20-\x7E]+$` —
//! `smithy_query(query="shape",shape="ClientToken")`: the shortest, the longest, and one
//! carrying spaces and punctuation), and the ways a later request can relate to the first
//! (byte-identical, a changed scalar, a nested structure added, a member dropped, a list
//! changed).
//!
//! The wording is asserted as the LITERAL sentence the service must answer, not through
//! the crate constant, so a change to that constant cannot quietly change what this suite
//! accepts. Nothing here can pass against a canned answer: the refusal has to name the id
//! the first call minted, the resource is read back afterwards (through
//! `GetWorkspaceInstance` / `ListWorkspaceInstances` for an instance, and out of the
//! engine for a volume, which this service models no operation to report back), and every
//! reused body is shown to be a create this service accepts by being sent under a token
//! of its own.

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;

use crate::idempotency_token_reuse::{stored_volumes, CONFLICT_STATUS};
use crate::volume_lifecycle::create_volume;
use crate::workspace_instance_lifecycle::{
    call, create, error_code, harness, launchable_managed_instance, list_all_ids, ok,
    TEST_IMAGE_ID, TEST_INSTANCE_TYPE,
};

/// The `Message` AWS answers a create carrying a spent `ClientToken` with: fixed text in
/// full, naming no id, token or parameter.
const SPENT_TOKEN_MESSAGE: &str = "The client token provided does not match the request";

/// One create and the later creates that come back under the SAME token.
///
/// `token` is the form of `ClientToken` this row drives, and `reuses` the later request
/// bodies, each with the way it relates to `first` as its case name.
struct SingleUseRow {
    case: &'static str,
    token: String,
    first: Value,
    reuses: Vec<(&'static str, Value)>,
}

/// `body` with `token` as its `ClientToken`.
fn under_token(body: &Value, token: &str) -> Value {
    let mut body = body.clone();
    body["ClientToken"] = json!(token);
    body
}

/// The refusal a create carrying a spent token must answer: the modelled status and code,
/// the id the token DID create, that resource's kind, and AWS's fixed wording.
fn assert_spent_token(case: &str, status: u16, payload: &Value, created: &str, kind: &str) {
    assert_eq!(
        status, CONFLICT_STATUS,
        "case {case:?}: expected HTTP {CONFLICT_STATUS} for a spent client token, got \
         {status} with {payload}"
    );
    assert_eq!(
        error_code(payload),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {payload}"
    );
    assert_eq!(
        payload["ResourceId"].as_str(),
        Some(created),
        "case {case:?}: expected the conflict to name the resource the token created \
         ({created:?}), got {payload}"
    );
    assert_eq!(
        payload["ResourceType"].as_str(),
        Some(kind),
        "case {case:?}: expected the conflicting resource's kind {kind:?}, got {payload}"
    );
    let message = payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str());
    assert_eq!(
        message,
        Some(SPENT_TOKEN_MESSAGE),
        "case {case:?}: expected {SPENT_TOKEN_MESSAGE:?}, got {payload}"
    );
}

/// The `CreateWorkspaceInstance` rows: the token forms the model admits, each with later
/// requests that relate to the first in a different way.
fn instance_rows() -> Vec<SingleUseRow> {
    vec![
        SingleUseRow {
            case: "the shortest token the model admits, reused on the identical request",
            token: "a".to_string(),
            first: json!({ "ManagedInstance": launchable_managed_instance() }),
            reuses: vec![(
                "the identical request",
                json!({ "ManagedInstance": launchable_managed_instance() }),
            )],
        },
        SingleUseRow {
            case: "the longest token the model admits, reused on a changed scalar",
            token: "t".repeat(64),
            first: json!({
                "ManagedInstance": { "ImageId": TEST_IMAGE_ID, "InstanceType": "t3.medium" },
            }),
            reuses: vec![
                (
                    "the identical request",
                    json!({
                        "ManagedInstance": { "ImageId": TEST_IMAGE_ID, "InstanceType": "t3.medium" },
                    }),
                ),
                (
                    "a different InstanceType",
                    json!({
                        "ManagedInstance": { "ImageId": TEST_IMAGE_ID, "InstanceType": "m5.large" },
                    }),
                ),
            ],
        },
        SingleUseRow {
            case: "a token carrying spaces and punctuation, reused with structure added",
            token: "token with spaces ~!@#$%^&*()".to_string(),
            first: json!({ "ManagedInstance": launchable_managed_instance() }),
            reuses: vec![
                (
                    "a nested Placement added",
                    json!({
                        "ManagedInstance": {
                            "ImageId": TEST_IMAGE_ID,
                            "InstanceType": TEST_INSTANCE_TYPE,
                            "Placement": { "AvailabilityZone": "us-east-1a" },
                        },
                    }),
                ),
                (
                    "a list-valued member added",
                    json!({
                        "ManagedInstance": {
                            "ImageId": TEST_IMAGE_ID,
                            "InstanceType": TEST_INSTANCE_TYPE,
                            "SecurityGroupIds": ["sg-00000001"],
                        },
                    }),
                ),
            ],
        },
        SingleUseRow {
            case: "a token spent on a tagged create, reused with the tags dropped",
            token: "tok-tagged".to_string(),
            first: json!({
                "ManagedInstance": launchable_managed_instance(),
                "Tags": [{ "Key": "env", "Value": "dev" }],
                "BillingConfiguration": { "BillingMode": "HOURLY" },
            }),
            reuses: vec![
                (
                    "the Tags dropped",
                    json!({ "ManagedInstance": launchable_managed_instance() }),
                ),
                (
                    "a different BillingMode",
                    json!({
                        "ManagedInstance": launchable_managed_instance(),
                        "Tags": [{ "Key": "env", "Value": "dev" }],
                        "BillingConfiguration": { "BillingMode": "MONTHLY" },
                    }),
                ),
            ],
        },
        SingleUseRow {
            case: "a token spent on one image, reused with another",
            token: "tok-image".to_string(),
            first: json!({
                "ManagedInstance": {
                    "ImageId": "ami-01111111111111111",
                    "InstanceType": TEST_INSTANCE_TYPE,
                },
            }),
            reuses: vec![(
                "a different ImageId",
                json!({
                    "ManagedInstance": {
                        "ImageId": "ami-02222222222222222",
                        "InstanceType": TEST_INSTANCE_TYPE,
                    },
                }),
            )],
        },
    ]
}

/// The `CreateVolume` rows, spanning the same dimensions over that operation's own
/// request: the token forms, and later requests identical to, bigger than, differently
/// typed from and differently placed than the first.
fn volume_rows() -> Vec<SingleUseRow> {
    let base = json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 10, "VolumeType": "gp3" });
    vec![
        SingleUseRow {
            case: "the shortest token, reused on the identical request",
            token: "v".to_string(),
            first: base.clone(),
            reuses: vec![("the identical request", base.clone())],
        },
        SingleUseRow {
            case: "the longest token, reused on a bigger volume",
            token: "v".repeat(64),
            first: base.clone(),
            reuses: vec![
                ("the identical request", base.clone()),
                (
                    "a different SizeInGB",
                    json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 500, "VolumeType": "gp3" }),
                ),
            ],
        },
        SingleUseRow {
            case: "a token carrying spaces, reused with another type and a member added",
            token: "volume token 1".to_string(),
            first: base.clone(),
            reuses: vec![
                (
                    "a different VolumeType",
                    json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 10, "VolumeType": "gp2" }),
                ),
                (
                    "an added Iops",
                    json!({
                        "AvailabilityZone": "us-east-1a",
                        "SizeInGB": 10,
                        "VolumeType": "gp3",
                        "Iops": 3000,
                    }),
                ),
            ],
        },
        SingleUseRow {
            case: "a token spent in one zone, reused in another",
            token: "tok-zone".to_string(),
            first: base,
            reuses: vec![(
                "a different Availability Zone",
                json!({ "AvailabilityZone": "us-east-1b", "SizeInGB": 10, "VolumeType": "gp3" }),
            )],
        },
    ]
}

#[tokio::test]
async fn a_spent_token_refuses_every_later_create_workspace_instance() {
    for row in instance_rows() {
        // One store per row, so what the store holds afterwards is this row's doing.
        let (router, _engine) = harness();
        let created = create(&router, under_token(&row.first, &row.token)).await;

        // Every later create under that token is refused — the first call's own request
        // included, and the same refusal however many times it arrives.
        let mut later: Vec<(&str, Value)> = vec![("the first request again", row.first.clone())];
        later.extend(row.reuses.iter().cloned());
        for (reuse, body) in &later {
            for attempt in 1..=2 {
                let case = format!("{} / {reuse} (attempt {attempt})", row.case);
                let (status, payload) =
                    call(&router, "CreateWorkspaceInstance", under_token(body, &row.token)).await;
                assert_spent_token(
                    &case,
                    status,
                    &payload,
                    &created,
                    wsinst::state::WORKSPACE_INSTANCE_RESOURCE_TYPE,
                );
            }
        }

        // Nothing those refusals asked for reached the store: the ONE instance the token
        // created is still the only one, and it still reads back.
        let ids = list_all_ids(&router, json!({})).await;
        assert_eq!(
            ids,
            vec![created.clone()],
            "case {:?}: expected the refusals to create no instance, got {ids:?}",
            row.case
        );
        let got = ok(
            &router,
            "GetWorkspaceInstance",
            json!({ "WorkspaceInstanceId": &created }),
        )
        .await;
        assert_eq!(
            got["WorkspaceInstanceId"].as_str(),
            Some(created.as_str()),
            "case {:?}: expected the created instance to still be readable, got {got}",
            row.case
        );

        // What makes the later calls conflicts is the SPENT token, not the bodies: each
        // of them under a token of its own is a create this service carries out. The
        // fresh token is short on purpose — the modelled `ClientToken` is `len 1..64`.
        for (index, (reuse, body)) in row.reuses.iter().enumerate() {
            let fresh = create(&router, under_token(body, &format!("fresh-{index}"))).await;
            assert_ne!(
                fresh, created,
                "case {:?} / {reuse}: expected its own instance under a fresh token, got \
                 {created:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_spent_token_refuses_every_later_create_volume() {
    for row in volume_rows() {
        let (router, engine) = harness();
        let created = create_volume(&router, under_token(&row.first, &row.token)).await;

        let mut later: Vec<(&str, Value)> = vec![("the first request again", row.first.clone())];
        later.extend(row.reuses.iter().cloned());
        for (reuse, body) in &later {
            for attempt in 1..=2 {
                let case = format!("{} / {reuse} (attempt {attempt})", row.case);
                let (status, payload) =
                    call(&router, "CreateVolume", under_token(body, &row.token)).await;
                assert_spent_token(
                    &case,
                    status,
                    &payload,
                    &created,
                    wsinst::state::VOLUME_RESOURCE_TYPE,
                );
            }
        }

        // The one volume the token created is still the only one, with the first call's
        // own configuration: no refusal wrote anything.
        let volumes = stored_volumes(&engine);
        assert_eq!(
            volumes.keys().collect::<Vec<_>>(),
            vec![&created],
            "case {:?}: expected the refusals to create no volume, got {:?}",
            row.case,
            volumes.keys().collect::<Vec<_>>()
        );
        let record = volumes
            .get(&created)
            .unwrap_or_else(|| panic!("case {:?}: the created volume must survive", row.case));
        assert_eq!(
            (
                record.availability_zone.as_str(),
                record.size_in_gb,
                record.volume_type.as_deref(),
            ),
            (
                row.first["AvailabilityZone"].as_str().unwrap_or_default(),
                row.first["SizeInGB"].as_i64().map(|size| size as i32),
                row.first["VolumeType"].as_str(),
            ),
            "case {:?}: expected the first call's configuration, got {record:?}",
            row.case
        );

        for (index, (reuse, body)) in row.reuses.iter().enumerate() {
            let fresh = create_volume(&router, under_token(body, &format!("fresh-{index}"))).await;
            assert_ne!(
                fresh, created,
                "case {:?} / {reuse}: expected its own volume under a fresh token, got \
                 {created:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_create_carrying_no_token_spends_nothing() {
    let (router, _engine) = harness();

    // The token is what a reuse is refused on, so repeated creates that carry NONE are
    // each their own request and each creates its own instance.
    let mut minted = Vec::new();
    for attempt in 1..=3 {
        let id = create(
            &router,
            json!({ "ManagedInstance": launchable_managed_instance() }),
        )
        .await;
        assert!(
            !minted.contains(&id),
            "case untokened create {attempt}: expected a fresh instance, got the \
             already-minted {id:?}"
        );
        minted.push(id);
    }

    let mut ids = list_all_ids(&router, json!({})).await;
    ids.sort();
    minted.sort();
    assert_eq!(
        ids, minted,
        "case untokened creates: expected one instance per call, got {ids:?}"
    );
}
