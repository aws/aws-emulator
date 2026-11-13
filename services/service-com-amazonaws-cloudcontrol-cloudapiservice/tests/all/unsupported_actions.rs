// SPDX-License-Identifier: CC0-1.0

//! An action against a resource type NO RESOURCE PROVIDER stands behind.
//!
//! The rule every case here holds the service to: **Cloud Control API acts on a resource
//! only through the handlers the resource type's provider publishes, so a type whose
//! provider publishes none of them — the provisioning class the CloudFormation registry
//! reports as `NON_PROVISIONABLE`, "the resource type doesn't include create, read, and
//! delete handlers, and therefore can't actually be provisioned" — has no handler for any
//! action to call, and every action against it is refused with the modelled
//! `UnsupportedActionException` (HTTP 405) naming the type the request carried and the
//! action the operation performs; a type a provider DOES stand behind is served as
//! before.**
//!
//! The first table is one row per `(type, operation)` pair: every type in that class
//! (`service_common::cfn_provisioning::NON_PROVISIONABLE_RESOURCE_TYPES`, the one catalogue
//! the registry reports the class from, so this suite cannot target a type the registry
//! publishes differently) against every operation of this service that names a `TypeName`.
//! The rows differ in the desired state and the identifier they carry too, because the
//! refusal is reached before either is resolved. The second table is the positive control —
//! including a type from the SAME namespace as the refused ones, so what is asserted is a
//! rule about the TYPE rather than about its namespace.

use serde_json::{json, Value};

use super::emulator::{code, descriptions, message, progress, text, Emulator};

/// A patch that changes nothing: a case about the type never reaches the patch.
fn inert_patch() -> Value {
    json!([{ "op": "add", "path": "/Tags", "value": [] }])
}

/// The desired states the refused creates carry, cycled over the rows: an empty resource
/// model, one naming a property the type would hold, and one carrying several — none of
/// which is read, because the type is resolved first.
fn desired_states() -> Vec<Value> {
    vec![
        json!({}),
        json!({ "Name": "whatever" }),
        json!({ "Tags": [], "Timeout": 300 }),
    ]
}

/// The identifiers the refused reads, updates and deletes carry, cycled the same way: a
/// plain name, a name with the punctuation an identifier may carry, and a long-ish one.
fn identifiers() -> Vec<&'static str> {
    vec![
        "does-not-matter",
        "stack/whatever/0123456789abcdef",
        "WhateverTheCallerHappenedToName",
    ]
}

#[tokio::test]
async fn every_action_against_a_type_with_no_provider_is_unsupported() {
    let emulator = Emulator::new();
    let catalog = ::service_common::cfn_provisioning::NON_PROVISIONABLE_RESOURCE_TYPES;
    assert!(
        !catalog.is_empty(),
        "expected the registry to publish at least one resource type no provider stands \
         behind, got {catalog:?}"
    );
    let states = desired_states();
    let identifiers = identifiers();

    for (row, type_name) in catalog.iter().enumerate() {
        let desired = &states[row % states.len()];
        let identifier = identifiers[row % identifiers.len()];
        // Every operation that names a type, with the action Cloud Control API performs
        // for it.
        let answers = [
            (
                "CreateResource",
                "CREATE",
                emulator.create(type_name, desired, None).await,
            ),
            (
                "GetResource",
                "READ",
                emulator.get(type_name, identifier).await,
            ),
            (
                "ListResources",
                "LIST",
                emulator
                    .cc(
                        "ListResources",
                        Emulator::list_request(type_name, None, None, None),
                    )
                    .await,
            ),
            (
                "UpdateResource",
                "UPDATE",
                emulator
                    .update(type_name, identifier, &inert_patch(), None)
                    .await,
            ),
            (
                "DeleteResource",
                "DELETE",
                emulator.delete(type_name, identifier).await,
            ),
        ];
        for (operation, action, (status, body)) in answers {
            let case = format!("{type_name} ({operation})");
            assert_eq!(
                status, 405,
                "case {case:?}: expected HTTP 405, got {status} with body {body:?}"
            );
            assert!(
                code(&body).contains("UnsupportedActionException"),
                "case {case:?}: expected UnsupportedActionException, got {:?}",
                code(&body)
            );
            assert_eq!(
                message(&body),
                format!("Resource type {type_name} does not support {action} action"),
                "case {case:?}: unexpected wording"
            );
            assert!(
                progress(&body).is_null(),
                "case {case:?}: a refused request mints no resource operation, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_type_a_provider_stands_behind_is_served() {
    let emulator = Emulator::new();
    // The rows differ along the owning service — including CloudFormation's own
    // `AWS::CloudFormation::Stack`, whose namespace is the one every refused row above
    // lives in — and along where the primary identifier comes from: a desired state that
    // carries it, and a type whose OWNING service assigns it (`AWS::SNS::Topic`'s
    // `TopicArn`, which its schema declares read-only). Each row names the model a read
    // must report: for a type whose resources live in the OWNING service's store
    // (`AWS::Logs::LogGroup` is a CloudWatch Logs log group) that is the model that service
    // holds, read-only properties included.
    let topic_arn =
        engine::emulator_arn("sns", super::emulator::SIGNING_REGION, "provisionable-topic");
    let rows = [
        (
            "AWS::SNS::Topic",
            json!({ "TopicName": "provisionable-topic" }),
            topic_arn.clone(),
            json!({ "TopicArn": topic_arn, "TopicName": "provisionable-topic" }),
        ),
        (
            "AWS::CloudFormation::Stack",
            json!({ "StackName": "provisionable-stack" }),
            "provisionable-stack".to_string(),
            json!({ "StackName": "provisionable-stack" }),
        ),
        (
            "AWS::Logs::LogGroup",
            json!({ "LogGroupName": "/provisionable/group" }),
            "/provisionable/group".to_string(),
            json!({
                "LogGroupName": "/provisionable/group",
                "LogGroupClass": "STANDARD",
                "Arn": engine::emulator_arn(
                    "logs",
                    super::emulator::SIGNING_REGION,
                    "log-group:/provisionable/group:*",
                ),
            }),
        ),
    ];

    for (type_name, desired, expected_identifier, expected) in rows {
        let identifier = emulator.provision(type_name, &desired).await;
        assert_eq!(
            identifier, expected_identifier,
            "case {type_name:?}: expected the identifier {expected_identifier:?}, got \
             {identifier:?}"
        );
        let stored = emulator.properties(type_name, &identifier).await;
        assert_eq!(
            stored, expected,
            "case {type_name:?}: expected the created resource to read back as \
             {expected:?}, got {stored:?}"
        );
        let listed = emulator.list(type_name, None, None).await;
        let identifiers: Vec<String> = descriptions(&listed)
            .iter()
            .map(|description| text(description, "Identifier"))
            .collect();
        assert_eq!(
            identifiers,
            vec![identifier.clone()],
            "case {type_name:?}: expected the inventory of the type to be the one resource \
             created, got {identifiers:?}"
        );
    }
}
