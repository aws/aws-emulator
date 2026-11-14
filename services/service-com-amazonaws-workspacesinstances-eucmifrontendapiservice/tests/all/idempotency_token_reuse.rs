// SPDX-License-Identifier: CC0-1.0

//! The idempotency-token rule of this service's `@idempotent` creates, driven over
//! the wire this service really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`).
//!
//! The rule under test: a create's `ClientToken` is SINGLE-USE. Once a create has
//! created a resource under a token, that token is spent: a later create carrying it is
//! refused with the modelled `ConflictException` (HTTP 409,
//! `smithy_query(query="shape",shape="ConflictException")`) naming the resource the token
//! did create — whether the later request's parameters differ from the first call's or are
//! byte-identical — and creates nothing, so the resource the token created keeps the
//! configuration the FIRST call asked for.
//!
//! Both operations the model gives a `ClientToken` `@idempotencyToken` are covered
//! (`CreateVolume` and `CreateWorkspaceInstance`), each over a table whose rows differ
//! along one dimension of the request: a scalar, an enum, a boolean, an added member,
//! a nested structure, and a list — plus the identical request, which is refused the
//! same way.
//!
//! Nothing here can pass against a canned answer: the refusal has to name the id the
//! first call minted, and what the store holds afterwards is
//! read back — through `GetWorkspaceInstance` / `ListWorkspaceInstances` /
//! `ListTagsForResource` for an instance, and straight out of the engine for a volume,
//! which this service models no operation to report back.
//!
//! The wire helpers come from the `workspace_instance_lifecycle` suite.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{
    call, error_code, harness, launchable_managed_instance, ok, DEFAULT_TEST_REGION,
    TEST_IMAGE_ID, TEST_INSTANCE_TYPE,
};

/// The HTTP status the model gives `ConflictException`
/// (`smithy_query(query="shape",shape="ConflictException")` → `httpError 409`).
pub(crate) const CONFLICT_STATUS: u16 = 409;

/// One pair of create calls that share a token: `first` is the call that spends it,
/// `second` the call that reuses it. `case` names the dimension they differ along.
///
/// `pub(crate)` with [`assert_conflict`]: the sibling `instance_token_parameter_mismatch`
/// suite drives the same rule over reuses that differ in SEVERAL members at once, and
/// states a row and checks a refusal the same way rather than restating either.
pub(crate) struct TokenRow {
    pub(crate) case: &'static str,
    pub(crate) first: Value,
    pub(crate) second: Value,
}

/// The volumes the store holds, keyed by id.
///
/// Read straight out of the engine the router was built from: this service models no
/// operation that reports a volume's configuration back, so the wire cannot show that
/// a refused retry left the first volume's configuration alone.
///
/// `pub(crate)` with [`assert_conflict`]: the sibling `client_token_single_use` suite
/// reads what a refused create left behind the same way.
pub(crate) fn stored_volumes(
    engine: &Arc<engine::InMemoryEngine>,
) -> BTreeMap<String, wsinst::state::VolumeRecord> {
    let state = wsinst::state::service_state(&**engine)
        .expect("the service store must be reachable from the engine the router was built from");
    let volumes = state.lock().volumes.clone();
    volumes
}

/// The volume idempotency claims the store holds, keyed by (Region, token): a claim is
/// scoped to the Region the create that spent the token was signed for, since the
/// volume it created belongs to that Region.
fn stored_volume_claims(
    engine: &Arc<engine::InMemoryEngine>,
) -> BTreeMap<(String, String), wsinst::state::IdempotencyClaim> {
    let state = wsinst::state::service_state(&**engine)
        .expect("the service store must be reachable from the engine the router was built from");
    let claims = state.lock().volume_client_tokens.clone();
    claims
}

/// `CreateVolume`, asserting a 200 and returning the minted id.
async fn create_volume(router: &MultiServiceRouter, body: Value) -> String {
    let payload = ok(router, "CreateVolume", body).await;
    payload["VolumeId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateVolume returned no VolumeId: {payload}"))
        .to_string()
}

/// The `ConflictException` a reused token must be refused with: the modelled status
/// and code, the id the token DID create, and that resource's kind.
pub(crate) fn assert_conflict(
    case: &str,
    status: u16,
    payload: &Value,
    created: &str,
    resource_type: &str,
) {
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
        Some(resource_type),
        "case {case:?}: expected the conflicting resource's kind, got {payload}"
    );
    // The service's fixed wording for this conflict: it states that the token does not
    // match the request and echoes neither the (`@sensitive`) token nor any other
    // submitted value.
    let message = payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str());
    assert_eq!(
        message,
        Some(wsinst::state::CLIENT_TOKEN_MISMATCH_MESSAGE),
        "case {case:?}: expected the service's client-token conflict wording, got {payload}"
    );
}

/// The `CreateVolume` pairs the rule is asserted over: each row reuses one token for a
/// request that differs from the first along ONE dimension, and every body on both
/// sides is a configuration this service accepts, so a refusal can only be the
/// conflict.
fn volume_mismatch_rows() -> Vec<TokenRow> {
    let base = json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 10, "VolumeType": "gp3" });
    vec![
        TokenRow {
            case: "a different Availability Zone",
            first: base.clone(),
            second: json!({ "AvailabilityZone": "us-east-1b", "SizeInGB": 10, "VolumeType": "gp3" }),
        },
        TokenRow {
            case: "a different SizeInGB",
            first: base.clone(),
            second: json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 500, "VolumeType": "gp3" }),
        },
        TokenRow {
            case: "a different VolumeType",
            first: base.clone(),
            second: json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 10, "VolumeType": "gp2" }),
        },
        TokenRow {
            case: "an added Iops",
            first: base.clone(),
            second: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "VolumeType": "gp3",
                "Iops": 3000,
            }),
        },
        TokenRow {
            case: "an added Throughput",
            first: base.clone(),
            second: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "VolumeType": "gp3",
                "Throughput": 125,
            }),
        },
        TokenRow {
            case: "a different Encrypted",
            first: base.clone(),
            second: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "VolumeType": "gp3",
                "Encrypted": true,
            }),
        },
        TokenRow {
            case: "a different KmsKeyId on an encrypted volume",
            first: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "Encrypted": true,
                "KmsKeyId": "arn:aws:kms:us-east-1:123456789012:key/aaaa",
            }),
            second: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "Encrypted": true,
                "KmsKeyId": "arn:aws:kms:us-east-1:123456789012:key/bbbb",
            }),
        },
        TokenRow {
            case: "an added SnapshotId",
            first: base.clone(),
            second: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "VolumeType": "gp3",
                "SnapshotId": "snap-0123456789abcdef",
            }),
        },
        TokenRow {
            case: "a different tag value",
            first: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "TagSpecifications": [{ "ResourceType": "volume", "Tags": [{ "Key": "env", "Value": "test" }] }],
            }),
            second: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "TagSpecifications": [{ "ResourceType": "volume", "Tags": [{ "Key": "env", "Value": "prod" }] }],
            }),
        },
        TokenRow {
            case: "one more tag in the list",
            first: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "TagSpecifications": [{ "ResourceType": "volume", "Tags": [{ "Key": "env", "Value": "test" }] }],
            }),
            second: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "TagSpecifications": [{
                    "ResourceType": "volume",
                    "Tags": [
                        { "Key": "env", "Value": "test" },
                        { "Key": "owner", "Value": "alice" },
                    ],
                }],
            }),
        },
        TokenRow {
            case: "the tags dropped altogether",
            first: json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 10,
                "TagSpecifications": [{ "ResourceType": "volume", "Tags": [{ "Key": "env", "Value": "test" }] }],
            }),
            second: json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 10 }),
        },
    ]
}

/// The `CreateWorkspaceInstance` pairs the rule is asserted over, spanning the same
/// dimensions this operation's request has: an added member, a changed scalar, a
/// nested structure, a list, and the billing choice.
fn instance_mismatch_rows() -> Vec<TokenRow> {
    vec![
        TokenRow {
            case: "configuration added to the minimal launchable ManagedInstance",
            first: json!({ "ManagedInstance": launchable_managed_instance() }),
            second: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": TEST_INSTANCE_TYPE,
                    "SubnetId": "subnet-0a1b2c3d",
                },
            }),
        },
        TokenRow {
            case: "a different InstanceType",
            first: json!({
                "ManagedInstance": { "ImageId": TEST_IMAGE_ID, "InstanceType": "t3.medium" },
            }),
            second: json!({
                "ManagedInstance": { "ImageId": TEST_IMAGE_ID, "InstanceType": "m5.large" },
            }),
        },
        TokenRow {
            case: "a different ImageId",
            first: json!({
                "ManagedInstance": {
                    "ImageId": "ami-01111111111111111",
                    "InstanceType": TEST_INSTANCE_TYPE,
                },
            }),
            second: json!({
                "ManagedInstance": {
                    "ImageId": "ami-02222222222222222",
                    "InstanceType": TEST_INSTANCE_TYPE,
                },
            }),
        },
        TokenRow {
            case: "a different nested Placement",
            first: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": TEST_INSTANCE_TYPE,
                    "Placement": { "AvailabilityZone": "us-east-1a", "Tenancy": "default" },
                },
            }),
            second: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": TEST_INSTANCE_TYPE,
                    "Placement": { "AvailabilityZone": "us-east-1b", "Tenancy": "default" },
                },
            }),
        },
        TokenRow {
            case: "a different list-valued member",
            first: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": TEST_INSTANCE_TYPE,
                    "SecurityGroupIds": ["sg-00000001"],
                },
            }),
            second: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": TEST_INSTANCE_TYPE,
                    "SecurityGroupIds": ["sg-00000001", "sg-00000002"],
                },
            }),
        },
        TokenRow {
            case: "different Tags",
            first: json!({
                "ManagedInstance": launchable_managed_instance(),
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
            second: json!({
                "ManagedInstance": launchable_managed_instance(),
                "Tags": [{ "Key": "env", "Value": "prod" }],
            }),
        },
        TokenRow {
            case: "a different BillingMode",
            first: json!({
                "ManagedInstance": launchable_managed_instance(),
                "BillingConfiguration": { "BillingMode": "HOURLY" },
            }),
            second: json!({
                "ManagedInstance": launchable_managed_instance(),
                "BillingConfiguration": { "BillingMode": "MONTHLY" },
            }),
        },
    ]
}

#[tokio::test]
async fn a_spent_volume_token_is_a_conflict_however_the_request_differs() {
    for (row_index, row) in volume_mismatch_rows().into_iter().enumerate() {
        // One store per row, so what the store holds afterwards is this row's doing.
        let (router, engine) = harness();
        let token = format!("tok-volume-{row_index}");

        let mut first = row.first.clone();
        first["ClientToken"] = json!(token);
        let created = create_volume(&router, first.clone()).await;

        let mut second = row.second.clone();
        second["ClientToken"] = json!(token);
        let (status, payload) = call(&router, "CreateVolume", second).await;
        assert_conflict(
            row.case,
            status,
            &payload,
            &created,
            wsinst::state::VOLUME_RESOURCE_TYPE,
        );

        // The refusal wrote nothing: the ONE volume the token created is still the one
        // the FIRST call asked for, with the first call's configuration.
        let volumes = stored_volumes(&engine);
        assert_eq!(
            volumes.len(),
            1,
            "case {:?}: expected the refused reuse to create no volume, got {:?}",
            row.case,
            volumes.keys().collect::<Vec<_>>()
        );
        let record = volumes
            .get(&created)
            .unwrap_or_else(|| panic!("case {:?}: the first volume must survive", row.case));
        assert_eq!(
            record.availability_zone.as_str(),
            first["AvailabilityZone"].as_str().unwrap_or_default(),
            "case {:?}: expected the first call's zone, got {record:?}",
            row.case
        );
        assert_eq!(
            record.size_in_gb,
            first["SizeInGB"].as_i64().map(|size| size as i32),
            "case {:?}: expected the first call's SizeInGB, got {record:?}",
            row.case
        );
        assert_eq!(
            record.volume_type.as_deref(),
            first["VolumeType"].as_str(),
            "case {:?}: expected the first call's VolumeType, got {record:?}",
            row.case
        );

        // The token is still the first request's: one claim, naming that volume — and
        // re-submitting the FIRST call's own body under it is refused too, the token
        // being spent rather than a key the first answer is replayed from.
        let claims = stored_volume_claims(&engine);
        assert_eq!(
            claims.len(),
            1,
            "case {:?}: expected the one claim the first call made, got {claims:?}",
            row.case
        );
        assert_eq!(
            claims
                .get(&(DEFAULT_TEST_REGION.to_string(), token.clone()))
                .map(|claim| claim.resource_id.as_str()),
            Some(created.as_str()),
            "case {:?}: expected the claim to still name {created:?}, got {claims:?}",
            row.case
        );
        let (status, payload) = call(&router, "CreateVolume", first).await;
        assert_conflict(
            &format!("{} (the identical request again)", row.case),
            status,
            &payload,
            &created,
            wsinst::state::VOLUME_RESOURCE_TYPE,
        );
        assert_eq!(
            stored_volumes(&engine).len(),
            1,
            "case {:?}: expected the identical reuse to create no volume either, got {:?}",
            row.case,
            stored_volumes(&engine).keys().collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn a_spent_instance_token_is_a_conflict_however_the_request_differs() {
    for (row_index, row) in instance_mismatch_rows().into_iter().enumerate() {
        let (router, _engine) = harness();
        let token = format!("tok-instance-{row_index}");

        let mut first = row.first.clone();
        first["ClientToken"] = json!(token);
        let payload = ok(&router, "CreateWorkspaceInstance", first.clone()).await;
        let created = payload["WorkspaceInstanceId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateWorkspaceInstance returned no id: {payload}"))
            .to_string();

        let mut second = row.second.clone();
        second["ClientToken"] = json!(token);
        let (status, payload) = call(&router, "CreateWorkspaceInstance", second).await;
        assert_conflict(
            row.case,
            status,
            &payload,
            &created,
            wsinst::state::WORKSPACE_INSTANCE_RESOURCE_TYPE,
        );

        // Exactly one instance exists, and it is the first call's: its ACTIVE billing
        // configuration and its tags are the ones that call asked for.
        let listed = ok(&router, "ListWorkspaceInstances", json!({})).await;
        let ids: Vec<&str> = listed["WorkspaceInstances"]
            .as_array()
            .unwrap_or_else(|| panic!("ListWorkspaceInstances returned nothing: {listed}"))
            .iter()
            .filter_map(|instance| instance["WorkspaceInstanceId"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec![created.as_str()],
            "case {:?}: expected only the first instance to exist, got {listed}",
            row.case
        );

        let got = ok(
            &router,
            "GetWorkspaceInstance",
            json!({ "WorkspaceInstanceId": &created }),
        )
        .await;
        let expected_billing_mode = first
            .get("BillingConfiguration")
            .and_then(|configuration| configuration["BillingMode"].as_str())
            .unwrap_or(wsinst::state::DEFAULT_BILLING_MODE);
        assert_eq!(
            got["BillingConfiguration"]["BillingMode"].as_str(),
            Some(expected_billing_mode),
            "case {:?}: expected the first call's billing mode, got {got}",
            row.case
        );

        let tags = ok(
            &router,
            "ListTagsForResource",
            json!({ "WorkspaceInstanceId": &created }),
        )
        .await;
        let stored: BTreeMap<&str, &str> = tags["Tags"]
            .as_array()
            .unwrap_or_else(|| panic!("ListTagsForResource returned no Tags: {tags}"))
            .iter()
            .filter_map(|tag| Some((tag["Key"].as_str()?, tag["Value"].as_str()?)))
            .collect();
        let expected: BTreeMap<&str, &str> = first
            .get("Tags")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|tag| Some((tag["Key"].as_str()?, tag["Value"].as_str()?)))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            stored, expected,
            "case {:?}: expected the first call's tags, got {tags}",
            row.case
        );

        // The token still belongs to the first request, which it is spent on: the FIRST
        // call's own body under it is refused too, and creates nothing.
        let (status, payload) = call(&router, "CreateWorkspaceInstance", first).await;
        assert_conflict(
            &format!("{} (the identical request again)", row.case),
            status,
            &payload,
            &created,
            wsinst::state::WORKSPACE_INSTANCE_RESOURCE_TYPE,
        );
        let listed = ok(&router, "ListWorkspaceInstances", json!({})).await;
        let ids: Vec<&str> = listed["WorkspaceInstances"]
            .as_array()
            .unwrap_or_else(|| panic!("ListWorkspaceInstances returned nothing: {listed}"))
            .iter()
            .filter_map(|instance| instance["WorkspaceInstanceId"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec![created.as_str()],
            "case {:?}: expected the identical reuse to create no instance either, got {listed}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_unchanged_request_is_refused_under_a_spent_token_however_its_body_is_written() {
    let (router, engine) = harness();

    // The token is spent whatever the bytes of the later request say: the same
    // parameters written in another member order are the same request, and a request
    // the token was never spent on is no different — both are refused.
    let rows = [
        (
            "the identical body",
            json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 10, "VolumeType": "gp3" }),
        ),
        (
            "the members in another order",
            json!({ "VolumeType": "gp3", "SizeInGB": 10, "AvailabilityZone": "us-east-1a" }),
        ),
        (
            "the members in a third order",
            json!({ "SizeInGB": 10, "AvailabilityZone": "us-east-1a", "VolumeType": "gp3" }),
        ),
    ];

    let token = "tok-volume-equivalent";
    let mut first = rows[0].1.clone();
    first["ClientToken"] = json!(token);
    let created = create_volume(&router, first).await;

    for (case, body) in &rows {
        let mut body = body.clone();
        body["ClientToken"] = json!(token);
        let (status, payload) = call(&router, "CreateVolume", body).await;
        assert_conflict(
            case,
            status,
            &payload,
            &created,
            wsinst::state::VOLUME_RESOURCE_TYPE,
        );
    }

    let volumes = stored_volumes(&engine);
    assert_eq!(
        volumes.len(),
        1,
        "case spent-token reuses: expected the one volume the token created, got {:?}",
        volumes.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn the_same_parameters_under_another_token_create_another_resource() {
    let (router, engine) = harness();
    let body = json!({ "AvailabilityZone": "us-east-1c", "SizeInGB": 8, "VolumeType": "gp2" });

    // The token is what makes two calls one request. The same parameters under
    // different tokens — and under no token at all — are different requests, so each
    // call creates its own volume.
    let mut created = Vec::new();
    for token in [Some("tok-a"), Some("tok-b"), None] {
        let mut request = body.clone();
        if let Some(token) = token {
            request["ClientToken"] = json!(token);
        }
        let id = create_volume(&router, request).await;
        assert!(
            !created.contains(&id),
            "case token {token:?}: expected a fresh volume, got the already-minted {id:?}"
        );
        created.push(id);
    }

    let volumes = stored_volumes(&engine);
    assert_eq!(
        volumes.len(),
        created.len(),
        "case distinct tokens: expected one volume per call, got {:?}",
        volumes.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_token_retired_by_delete_carries_no_claim_into_the_next_create() {
    let (router, engine) = harness();
    let token = "tok-volume-retired";
    let mut first = json!({ "AvailabilityZone": "us-east-1a", "SizeInGB": 10, "VolumeType": "gp3" });
    first["ClientToken"] = json!(token);
    let created = create_volume(&router, first).await;

    // `DeleteVolume` retires the claim with the record, so the token is unspent again:
    // a later call carrying it is a new request, not a conflict with a volume that no
    // longer exists.
    ok(&router, "DeleteVolume", json!({ "VolumeId": &created })).await;
    assert!(
        stored_volume_claims(&engine).is_empty(),
        "case after delete: expected the claim to be retired, got {:?}",
        stored_volume_claims(&engine)
    );

    let mut different = json!({ "AvailabilityZone": "us-east-1b", "SizeInGB": 500, "VolumeType": "io2", "Iops": 100 });
    different["ClientToken"] = json!(token);
    let second = create_volume(&router, different).await;
    assert_ne!(
        second, created,
        "case after delete: expected a new volume, got the deleted {created:?}"
    );
}
