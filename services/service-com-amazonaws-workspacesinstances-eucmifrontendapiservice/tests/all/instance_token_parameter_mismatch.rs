// SPDX-License-Identifier: CC0-1.0

//! `CreateWorkspaceInstance` when a spent `ClientToken` comes back on a request that
//! differs in SEVERAL members at once, driven over the wire this service speaks
//! (awsJson1_0, `POST /`, `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`).
//!
//! The rule under test: a create's `ClientToken` is spent by the create that carried it,
//! so a later create carrying that token is refused with the modelled
//! `ConflictException` (HTTP 409, `ResourceId`/`ResourceType` naming the resource the
//! token did create — `smithy_query(query="shape",shape="ConflictException")`) however the
//! request differs from the first — in ANY member, at any depth, in any number — while
//! the resource that token created keeps the configuration the FIRST call asked for.
//!
//! The sibling `idempotency_token_reuse` suite varies ONE member per row; the rows
//! here vary a member nested two levels down, a member dropped, members added, a list
//! and a scalar together, and a whole structure replaced — the shapes of difference a
//! refusal written for the members it expected would miss.
//!
//! Nothing here can pass against a canned answer: the refusal has to name the id the
//! first call minted, the configuration the store holds for it is read back after the
//! refusal, and each refused body is shown to be a create this service accepts by
//! being sent under a token of its own.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;

use crate::idempotency_token_reuse::{assert_conflict, TokenRow};
use crate::workspace_instance_lifecycle::{
    call, create, harness, launchable_managed_instance, ok, DEFAULT_TEST_REGION, TEST_IMAGE_ID,
    TEST_INSTANCE_TYPE,
};

/// Everything the store holds about instance `id` that a create call decided,
/// rendered field by field.
///
/// The `ManagedInstance` a create persists is reported by no operation of this slice,
/// so reading the record is the only way to show that a refused retry left the first
/// call's configuration — not just its billing mode and tags — exactly as it was.
fn stored_configuration(engine: &Arc<engine::InMemoryEngine>, case: &str, id: &str) -> String {
    let store = wsinst::state::service_state(&**engine).expect("the service's own store");
    let state = store.lock();
    let record = state
        .instances
        .get(id)
        .unwrap_or_else(|| panic!("case {case:?}: the store must hold the created {id:?}"));
    format!(
        "region={:?} ec2_instance_id={:?} managed_instance={:?} billing_configuration={:?} \
         tags={:?} client_token={:?}",
        record.region,
        record.ec2_instance_id,
        record.managed_instance,
        record.billing_configuration,
        record.tags,
        record.client_token,
    )
}

/// The instance ids the store holds, in id order.
fn stored_ids(engine: &Arc<engine::InMemoryEngine>) -> Vec<String> {
    let store = wsinst::state::service_state(&**engine).expect("the service's own store");
    let ids = store.lock().instances.keys().cloned().collect();
    ids
}

/// The create pairs the rule is asserted over. Every member named below is one the
/// model declares on `CreateWorkspaceInstance`'s input or on `ManagedInstanceRequest`
/// (`smithy_query(query="shape",shape="ManagedInstanceRequest")`), and every value
/// satisfies that member's constraints, so a refusal of either side can only be the
/// conflict and never a validation failure — which the second test here proves by
/// sending each `second` body under a token of its own.
fn mismatch_rows() -> Vec<TokenRow> {
    vec![
        TokenRow {
            case: "three members at once: a changed InstanceType, a BillingConfiguration \
                   added and the Tags dropped",
            first: json!({
                "ManagedInstance": { "ImageId": TEST_IMAGE_ID, "InstanceType": "t3.medium" },
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
            second: json!({
                "ManagedInstance": { "ImageId": TEST_IMAGE_ID, "InstanceType": "m5.large" },
                "BillingConfiguration": { "BillingMode": "MONTHLY" },
            }),
        },
        TokenRow {
            case: "one member two levels down: Placement.Tenancy",
            // `m5`, whose catalog row publishes both tenancies, so either side is a
            // configuration this service launches.
            first: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": "m5.large",
                    "Placement": { "AvailabilityZone": "us-east-1a", "Tenancy": "default" },
                },
            }),
            second: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": "m5.large",
                    "Placement": { "AvailabilityZone": "us-east-1a", "Tenancy": "dedicated" },
                },
            }),
        },
        TokenRow {
            case: "a member dropped from ManagedInstance",
            first: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": TEST_INSTANCE_TYPE,
                    "SubnetId": "subnet-0a1b2c3d",
                },
            }),
            second: json!({ "ManagedInstance": launchable_managed_instance() }),
        },
        TokenRow {
            case: "two members added, a boolean and an integer",
            first: json!({ "ManagedInstance": launchable_managed_instance() }),
            second: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": TEST_INSTANCE_TYPE,
                    "EbsOptimized": true,
                    "Ipv6AddressCount": 2,
                },
            }),
        },
        TokenRow {
            case: "a list one entry longer and a tag value changed together",
            first: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": TEST_INSTANCE_TYPE,
                    "SecurityGroupIds": ["sg-0000000a"],
                },
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
            second: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": TEST_INSTANCE_TYPE,
                    "SecurityGroupIds": ["sg-0000000a", "sg-0000000b"],
                },
                "Tags": [{ "Key": "env", "Value": "staging" }],
            }),
        },
        TokenRow {
            case: "the whole ManagedInstance replaced by the minimal launchable one",
            first: json!({
                "ManagedInstance": {
                    "ImageId": "ami-01111111111111111",
                    "InstanceType": "c5.large",
                    "SubnetId": "subnet-0a1b2c3d",
                },
            }),
            second: json!({ "ManagedInstance": launchable_managed_instance() }),
        },
        TokenRow {
            case: "a tag nested inside ManagedInstance.TagSpecifications",
            first: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": TEST_INSTANCE_TYPE,
                    "TagSpecifications": [{
                        "ResourceType": "instance",
                        "Tags": [{ "Key": "team", "Value": "research" }],
                    }],
                },
            }),
            second: json!({
                "ManagedInstance": {
                    "ImageId": TEST_IMAGE_ID,
                    "InstanceType": TEST_INSTANCE_TYPE,
                    "TagSpecifications": [{
                        "ResourceType": "instance",
                        "Tags": [{ "Key": "team", "Value": "platform" }],
                    }],
                },
            }),
        },
    ]
}

#[tokio::test]
async fn a_spent_instance_token_is_a_conflict_on_a_request_differing_in_several_members() {
    for (row_index, row) in mismatch_rows().into_iter().enumerate() {
        // One store per row, so what the store holds afterwards is this row's doing.
        let (router, engine) = harness();
        let token = format!("tok-multi-{row_index}");

        let mut first = row.first.clone();
        first["ClientToken"] = json!(token);
        let created = create(&router, first.clone()).await;
        let configuration = stored_configuration(&engine, row.case, &created);

        // The reuse is refused the same way however many times it arrives: the token
        // stays spent on the FIRST request, so the second attempt is the same conflict
        // as the first and neither of them writes.
        let mut second = row.second.clone();
        second["ClientToken"] = json!(token);
        for attempt in 1..=2 {
            let (status, payload) = call(&router, "CreateWorkspaceInstance", second.clone()).await;
            assert_conflict(
                &format!("{} (attempt {attempt})", row.case),
                status,
                &payload,
                &created,
                wsinst::state::WORKSPACE_INSTANCE_RESOURCE_TYPE,
            );
        }

        // Nothing the refused call asked for reached the store: one instance, and every
        // field of it still the one the first call decided — including the
        // `ManagedInstance` configuration no operation of this slice reports back.
        assert_eq!(
            stored_ids(&engine),
            vec![created.clone()],
            "case {:?}: expected the refused retry to create no instance, got {:?}",
            row.case,
            stored_ids(&engine)
        );
        assert_eq!(
            stored_configuration(&engine, row.case, &created),
            configuration,
            "case {:?}: expected the first call's configuration to survive the refusal",
            row.case
        );

        // The claim still names that instance, so the FIRST request is refused under
        // that token too, and the instance is still readable.
        let claims = {
            let store = wsinst::state::service_state(&*engine).expect("the service's own store");
            let claims = store.lock().client_tokens.clone();
            claims
        };
        assert_eq!(
            claims
                .get(&(DEFAULT_TEST_REGION.to_string(), token.clone()))
                .map(|claim| claim.resource_id.as_str()),
            Some(created.as_str()),
            "case {:?}: expected the claim to still name {created:?}, got {claims:?}",
            row.case
        );
        let (status, payload) = call(&router, "CreateWorkspaceInstance", first).await;
        assert_conflict(
            &format!("{} (the first request again)", row.case),
            status,
            &payload,
            &created,
            wsinst::state::WORKSPACE_INSTANCE_RESOURCE_TYPE,
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
            "case {:?}: expected the instance to still be readable, got {got}",
            row.case
        );
        assert_eq!(
            stored_ids(&engine),
            vec![created.clone()],
            "case {:?}: expected the refusals to create no second instance, got {:?}",
            row.case,
            stored_ids(&engine)
        );
    }
}

#[tokio::test]
async fn every_refused_body_is_a_create_this_service_accepts_under_its_own_token() {
    for (row_index, row) in mismatch_rows().into_iter().enumerate() {
        let (router, engine) = harness();

        // What makes the retry a conflict is the SPENT token, not the body: the same
        // two bodies under tokens of their own are two different requests, so each
        // creates its own instance.
        let mut first = row.first.clone();
        first["ClientToken"] = json!(format!("tok-left-{row_index}"));
        let left = create(&router, first).await;

        let mut second = row.second.clone();
        second["ClientToken"] = json!(format!("tok-right-{row_index}"));
        let right = create(&router, second.clone()).await;
        assert_ne!(
            left, right,
            "case {:?}: expected the second body to mint its own instance under its own token",
            row.case
        );

        // And under no token at all it is a different request again.
        second
            .as_object_mut()
            .unwrap_or_else(|| panic!("case {:?}: the row body must be an object", row.case))
            .remove("ClientToken");
        let untokened = create(&router, second).await;
        let mut expected = vec![left, right, untokened];
        expected.sort();
        assert_eq!(
            stored_ids(&engine),
            expected,
            "case {:?}: expected one instance per request, got {:?}",
            row.case,
            stored_ids(&engine)
        );
    }
}
