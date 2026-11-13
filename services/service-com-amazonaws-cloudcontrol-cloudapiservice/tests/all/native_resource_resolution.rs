// SPDX-License-Identifier: CC0-1.0

//! Where Cloud Control API resolves a resource.
//!
//! The rule every case here holds the service to: **Cloud Control API is a front end over
//! the resource type's own provider rather than an inventory of its own, so for a type
//! whose resources live in another service of this emulator every resource operation
//! resolves and acts on the resource in THAT service's state — "you can use this action to
//! return information about an existing resource in your account and Amazon Web Services
//! Region, whether those resources were provisioned using Cloud Control API"
//! (`GetResource`) — and a resource that service no longer holds is reported with the
//! modelled `ResourceNotFoundException` (HTTP 404), whichever API removed it.**
//!
//! The rows differ along what the rule generalises over: which API the resource came into
//! being through, which API changed or removed it, which resource TYPE of the owning
//! service it is, and which properties the resource carries. `AWS::Logs::LogGroup` and
//! `AWS::Logs::Destination` are the types with a provider behind them here, and
//! `AWS::Kinesis::Stream` is the control for a type whose resources have nowhere else to
//! live, so what is asserted is a rule about where a resource LIVES rather than about one
//! type name.

use serde_json::{json, Value};

use super::emulator::{
    code, descriptions, message, properties_of, text, Emulator, STATUS_FAILED,
};
use super::refusal_wording::{
    assert_handler_not_found, generic_already_exists, generic_not_found,
};

/// The not-found diagnostic the handler of each type here states: Cloud Control's own
/// sentence, naming the resource as that HANDLER names it — for `AWS::Logs::LogGroup` the
/// type's primary-identifier object, which is the form its recorded refusal carries, and
/// for the other types the identifier the request carried.
fn not_found_detail(type_name: &str, identifier: &str) -> String {
    if type_name == LOG_GROUP {
        let rendered = json!({ "/properties/LogGroupName": identifier }).to_string();
        return generic_not_found(type_name, &rendered);
    }
    generic_not_found(type_name, identifier)
}

/// The type whose resources live in the CloudWatch Logs service's own store.
const LOG_GROUP: &str = "AWS::Logs::LogGroup";

/// A SECOND type of that same owning service, so a case is about where a resource lives
/// rather than about one type: a logical destination, which `logs:PutDestination` creates.
const DESTINATION: &str = "AWS::Logs::Destination";

/// The control: a type no provider of another service stands behind, whose resources live
/// in this service's own store.
const STREAM: &str = "AWS::Kinesis::Stream";

/// The model a read must report for the log group `name` under `retention`: its identity,
/// the class every group carries, the read-only ARN the type's schema declares and the
/// retention where the group is under one.
///
/// The ARN is built by the OWNING service's own builder, so a case pins the group Cloud
/// Control API resolved to the one CloudWatch Logs holds rather than to a string this
/// suite spells.
fn expected_model(name: &str, retention: Option<i64>) -> Value {
    let mut model = json!({
        "LogGroupName": name,
        "LogGroupClass": "STANDARD",
        "Arn": engine::emulator_arn(
            "logs",
            super::emulator::SIGNING_REGION,
            &format!("log-group:{name}:*"),
        ),
    });
    if let Some(days) = retention {
        model["RetentionInDays"] = json!(days);
    }
    model
}

/// Every log group name the OWNING service's own reader reports.
async fn native_groups(emulator: &Emulator) -> Vec<String> {
    let described = emulator.logs_ok("DescribeLogGroups", json!({})).await;
    described["logGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|group| text(group, "logGroupName"))
        .collect()
}

#[tokio::test]
async fn a_resource_the_owning_service_holds_is_the_one_cloud_control_reports() {
    let emulator = Emulator::with_cloudwatch_logs();
    // The rows differ along the properties the natively created resource carries — none
    // beyond its identity, a retention, a tag set, and a retention at the top of the
    // published catalogue — because the model a read reports is the owning record's.
    let rows: [(&str, Value, Value); 4] = [
        (
            "no property beyond the identity",
            json!({ "logGroupName": "native-plain" }),
            expected_model("native-plain", None),
        ),
        (
            "a retention",
            json!({ "logGroupName": "native-retained" }),
            expected_model("native-retained", Some(14)),
        ),
        (
            "a tag set",
            json!({ "logGroupName": "native-tagged", "tags": { "team": "platform" } }),
            {
                let mut model = expected_model("native-tagged", None);
                model["Tags"] = json!([{ "Key": "team", "Value": "platform" }]);
                model
            },
        ),
        (
            "a name with the path separators a log group name may carry",
            json!({ "logGroupName": "/aws-emulator/native/nested" }),
            expected_model("/aws-emulator/native/nested", Some(3653)),
        ),
    ];

    for (case, create, expected) in &rows {
        let name = text(create, "logGroupName");
        // Created over the OWNING service's own API: nothing of this group was ever
        // named to Cloud Control API.
        emulator.logs_ok("CreateLogGroup", create.clone()).await;
        if let Some(days) = expected["RetentionInDays"].as_i64() {
            emulator
                .logs_ok(
                    "PutRetentionPolicy",
                    json!({ "logGroupName": name, "retentionInDays": days }),
                )
                .await;
        }

        let (status, body) = emulator.get(LOG_GROUP, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResource must resolve a group the owning service holds, got \
             {status} with body {body:?}"
        );
        assert_eq!(
            text(&body, "TypeName"),
            LOG_GROUP,
            "case {case:?}: expected the type the request named, got {body:?}"
        );
        let reported = properties_of(&body["ResourceDescription"]);
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected the model the owning service holds, {expected:?}, got \
             {reported:?}"
        );
        assert_eq!(
            text(&body["ResourceDescription"], "Identifier"),
            name,
            "case {case:?}: expected the group's own identifier, got {body:?}"
        );
    }

    // And the listing of the type is the owning service's inventory, in identifier order.
    let mut expected: Vec<String> = rows
        .iter()
        .map(|(_, create, _)| text(create, "logGroupName"))
        .collect();
    expected.sort();
    assert_eq!(
        emulator.walk(LOG_GROUP, None).await,
        expected,
        "ListResources must report the resources the owning service holds"
    );
}

#[tokio::test]
async fn a_resource_the_owning_service_no_longer_holds_is_reported_as_not_found() {
    let emulator = Emulator::with_cloudwatch_logs();
    // The rows differ along how the group came into being and how it stopped existing:
    // each pair of APIs has to agree about the one resource.
    let rows = [
        ("created and deleted natively", false, false),
        ("created natively, deleted through Cloud Control", false, true),
        ("created through Cloud Control, deleted natively", true, false),
        (
            "created and deleted through Cloud Control",
            true,
            true,
        ),
    ];

    for (row, (case, create_through_cc, delete_through_cc)) in rows.into_iter().enumerate() {
        // One group per row, so no row depends on what another left behind.
        let name = format!("lifecycle-{row}");
        if create_through_cc {
            emulator
                .provision(LOG_GROUP, &json!({ "LogGroupName": name }))
                .await;
        } else {
            emulator
                .logs_ok("CreateLogGroup", json!({ "logGroupName": name }))
                .await;
        }
        // Whichever API created it, BOTH report it: a create through Cloud Control API is
        // a resource of the owning service, and one through the owning service's API is a
        // resource Cloud Control API reads.
        let (status, body) = emulator.get(LOG_GROUP, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResource must resolve the created group, got {status} with \
             body {body:?}"
        );
        assert!(
            native_groups(&emulator).await.contains(&name),
            "case {case:?}: the owning service must hold the created group, it holds {:?}",
            native_groups(&emulator).await
        );

        if delete_through_cc {
            emulator.delete_and_settle(LOG_GROUP, &name).await;
        } else {
            emulator
                .logs_ok("DeleteLogGroup", json!({ "logGroupName": name }))
                .await;
        }

        let (status, body) = emulator.get(LOG_GROUP, &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: GetResource after the delete: expected HTTP 404, got {status} \
             with body {body:?}"
        );
        assert!(
            code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            code(&body)
        );
        // The handler that owns a log group states a not-found in Cloud Control API's own
        // terms, which names the type and the resource's primary identifier as the
        // registry renders one, inside the handler envelope every such verdict carries.
        let request_token = assert_handler_not_found(
            &message(&body),
            LOG_GROUP,
            &not_found_detail(LOG_GROUP, &name),
            case,
        );
        assert!(
            !request_token.is_empty(),
            "case {case:?}: the refusal must name the handler invocation that failed"
        );
        assert!(
            !native_groups(&emulator).await.contains(&name),
            "case {case:?}: the owning service must hold the group no longer, it holds {:?}",
            native_groups(&emulator).await
        );
        let listed = emulator.walk(LOG_GROUP, None).await;
        assert!(
            !listed.contains(&name),
            "case {case:?}: the deleted group must not be listed, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_change_either_api_makes_is_the_change_the_other_reports() {
    let emulator = Emulator::with_cloudwatch_logs();
    let name = "shared-retention";
    emulator
        .logs_ok("CreateLogGroup", json!({ "logGroupName": name }))
        .await;

    // A change the OWNING service's API makes is what the next Cloud Control read
    // reports: a read is of the live resource, not of a snapshot taken when it was first
    // resolved.
    for days in [1_i64, 30, 3653] {
        emulator
            .logs_ok(
                "PutRetentionPolicy",
                json!({ "logGroupName": name, "retentionInDays": days }),
            )
            .await;
        let reported = emulator.properties(LOG_GROUP, name).await;
        assert_eq!(
            reported,
            expected_model(name, Some(days)),
            "retention {days}: expected the retention the owning service holds, got \
             {reported:?}"
        );
    }

    // And a change Cloud Control API makes is what the OWNING service's own reader
    // reports.
    let patched = emulator
        .update_and_settle(
            LOG_GROUP,
            name,
            &json!([{ "op": "replace", "path": "/RetentionInDays", "value": 90 }]),
        )
        .await;
    assert_eq!(
        serde_json::from_str::<Value>(&text(&patched, "ResourceModel")).unwrap_or(Value::Null),
        expected_model(name, Some(90)),
        "the update must report the model the owning service holds, got {patched:?}"
    );
    let described = emulator
        .logs_ok("DescribeLogGroups", json!({ "logGroupNamePrefix": name }))
        .await;
    assert_eq!(
        described["logGroups"][0]["retentionInDays"],
        json!(90),
        "the owning service's reader must report the retention the update set, got \
         {described:?}"
    );
}

#[tokio::test]
async fn a_create_of_a_resource_the_owning_service_already_holds_is_the_owning_verdict() {
    let emulator = Emulator::with_cloudwatch_logs();
    let name = "already-native";
    emulator
        .logs_ok("CreateLogGroup", json!({ "logGroupName": name }))
        .await;

    // The create is accepted like any other and the owning handler finds the resource
    // when the work falls due, so the caller polls it to a terminal FAILED carrying that
    // handler's error code and Cloud Control API's own wording for a resource that is
    // already there.
    let accepted = emulator
        .create_ok(LOG_GROUP, &json!({ "LogGroupName": name }), None)
        .await;
    let settled = emulator.terminal(&text(&accepted, "RequestToken")).await;
    assert_eq!(
        text(&settled, "OperationStatus"),
        STATUS_FAILED,
        "a create of a group the owning service already holds must fail, got {settled:?}"
    );
    assert_eq!(
        text(&settled, "ErrorCode"),
        "AlreadyExists",
        "unexpected handler error code, got {settled:?}"
    );
    assert_eq!(
        text(&settled, "StatusMessage"),
        generic_already_exists(LOG_GROUP, name),
        "unexpected wording, got {settled:?}"
    );
    // The refused create left the group exactly as the owning service held it.
    assert_eq!(
        emulator.properties(LOG_GROUP, name).await,
        expected_model(name, None),
        "a refused create must not change the resource"
    );
}

#[tokio::test]
async fn a_model_the_owning_service_cannot_hold_is_refused_rather_than_dropped() {
    let emulator = Emulator::with_cloudwatch_logs();
    // The rows differ along which rule of the OWNING service the model breaks: a name
    // outside the domain its model gives one, a name in the prefix it keeps for its own
    // groups, a retention outside the published catalogue, a class outside the published
    // set, a property of the type's schema this emulator's handler does not apply, and a
    // property the type does not declare at all.
    let rows = [
        (
            "a name outside the domain the owning service holds",
            json!({ "LogGroupName": "refused name" }),
            "LogGroupName must be 1 to 512 characters of letters, numbers and `. - _ / #`",
        ),
        (
            "a name in the prefix the owning service keeps",
            json!({ "LogGroupName": "aws/refused" }),
            "logGroupName must not start with the string 'aws/'",
        ),
        (
            "a retention outside the published catalogue",
            json!({ "LogGroupName": "refused-retention", "RetentionInDays": 2 }),
            "Invalid retention value.",
        ),
        (
            "a class outside the published set",
            json!({ "LogGroupName": "refused-class", "LogGroupClass": "PLATINUM" }),
            "Invalid logGroupClass value.",
        ),
        (
            "a key whose grant only KMS can answer for",
            json!({ "LogGroupName": "refused-key", "KmsKeyId": "arn:aws:kms:us-east-1:111122223333:key/k" }),
            "KmsKeyId is a property of AWS::Logs::LogGroup this emulator's handler does not \
             apply",
        ),
        (
            "a document the owning service installs with a call of its own",
            json!({ "LogGroupName": "refused-document", "DataProtectionPolicy": {} }),
            "DataProtectionPolicy is a property of AWS::Logs::LogGroup this emulator's \
             handler does not apply",
        ),
        (
            "a tag key the owning service keeps for itself",
            json!({
                "LogGroupName": "refused-tag",
                "Tags": [{ "Key": "aws:billing", "Value": "no" }],
            }),
            "Tags starting with 'AWS:' are reserved for AWS use.",
        ),
        (
            "a property the type does not declare",
            json!({ "LogGroupName": "refused-property", "Flavour": "vanilla" }),
            "Flavour is not a property of AWS::Logs::LogGroup",
        ),
    ];

    for (case, desired, expected) in rows {
        let identifier = text(&desired, "LogGroupName");
        let accepted = emulator.create_ok(LOG_GROUP, &desired, None).await;
        let settled = emulator.terminal(&text(&accepted, "RequestToken")).await;
        assert_eq!(
            text(&settled, "OperationStatus"),
            STATUS_FAILED,
            "case {case:?}: a model the owning service cannot hold must not succeed, got \
             {settled:?}"
        );
        assert_eq!(
            text(&settled, "ErrorCode"),
            "InvalidRequest",
            "case {case:?}: unexpected handler error code, got {settled:?}"
        );
        assert!(
            text(&settled, "StatusMessage").starts_with(expected),
            "case {case:?}: expected a message beginning {expected:?}, got {settled:?}"
        );
        // Nothing was created, in either store.
        let (status, body) = emulator.get(LOG_GROUP, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must leave no resource, got {status} with body \
             {body:?}"
        );
        assert!(
            !native_groups(&emulator).await.contains(&identifier),
            "case {case:?}: a refused create must leave the owning service holding nothing"
        );
    }
}

#[tokio::test]
async fn a_type_with_nowhere_else_to_live_is_served_from_this_services_own_store() {
    let emulator = Emulator::with_cloudwatch_logs();
    // The control for the rule: the resources of a type no other service of this emulator
    // owns live in this service's own store, so the model a read reports is the one the
    // create carried — and a log group of the SAME identifier is a different resource.
    let desired = json!({ "StreamName": "both", "ShardCount": 2 });
    assert_eq!(emulator.provision(STREAM, &desired).await, "both");
    emulator
        .logs_ok("CreateLogGroup", json!({ "logGroupName": "both" }))
        .await;

    assert_eq!(
        emulator.properties(STREAM, "both").await,
        desired,
        "a type with nowhere else to live must report the model it was created with"
    );
    assert_eq!(
        emulator.properties(LOG_GROUP, "both").await,
        expected_model("both", None),
        "the log group of the same identifier must report its own model"
    );

    // Deleting the stream leaves the group, which is what makes them two resources in two
    // stores.
    emulator.delete_and_settle(STREAM, "both").await;
    let (status, body) = emulator.get(STREAM, "both").await;
    assert_eq!(
        status, 404,
        "the deleted stream: expected HTTP 404, got {status} with body {body:?}"
    );
    let listed = emulator.list(LOG_GROUP, None, None).await;
    assert_eq!(
        descriptions(&listed).len(),
        1,
        "the log group must survive the stream's delete, got {listed:?}"
    );
}

#[tokio::test]
async fn a_delete_the_owning_service_refuses_leaves_the_resource_where_it_is() {
    let emulator = Emulator::with_cloudwatch_logs();
    let name = "protected-group";
    emulator
        .logs_ok("CreateLogGroup", json!({ "logGroupName": name }))
        .await;
    emulator
        .logs_ok(
            "PutLogGroupDeletionProtection",
            json!({ "logGroupIdentifier": name, "deletionProtectionEnabled": true }),
        )
        .await;

    // The rules the resource is under are the OWNING service's: deletion protection
    // "blocks all deletion operations until it is explicitly disabled", so the delete
    // terminates FAILED carrying that service's own diagnostic — under the handler error
    // code for an error the downstream service raised that maps to no other verdict — and
    // the group stays.
    let accepted = emulator.delete(LOG_GROUP, name).await;
    assert_eq!(
        accepted.0, 200,
        "the delete must be accepted, got {accepted:?}"
    );
    let settled = emulator
        .terminal(&text(&super::emulator::progress(&accepted.1), "RequestToken"))
        .await;
    assert_eq!(
        text(&settled, "OperationStatus"),
        STATUS_FAILED,
        "a delete the owning service refuses must not report success, got {settled:?}"
    );
    assert_eq!(
        text(&settled, "ErrorCode"),
        "GeneralServiceException",
        "unexpected handler error code, got {settled:?}"
    );
    assert_eq!(
        text(&settled, "StatusMessage"),
        "LogGroup has delete protection enabled. Disable delete protection before attempting \
         to delete the resource.",
        "unexpected wording, got {settled:?}"
    );
    assert_eq!(
        emulator.properties(LOG_GROUP, name).await,
        expected_model(name, None),
        "the resource must stay exactly as the owning service holds it"
    );
    assert!(
        native_groups(&emulator).await.contains(&name.to_string()),
        "the owning service must still hold the group"
    );
}

#[tokio::test]
async fn a_second_type_of_the_same_owning_service_resolves_the_same_way() {
    let emulator = Emulator::with_cloudwatch_logs();
    // The rule is about where a resource lives, not about one type: a logical destination
    // lives in the same owning service's store, so the same four lifecycles hold for it.
    let target = engine::emulator_arn("kinesis", super::emulator::SIGNING_REGION, "stream/feed");
    let role = format!("arn:aws:iam::{}:role/logs-to-kinesis", engine::EMULATOR_ACCOUNT_ID);

    // Created over the OWNING service's own API, resolved through Cloud Control API.
    let native = "native-destination";
    emulator
        .logs_ok(
            "PutDestination",
            json!({
                "destinationName": native,
                "targetArn": target,
                "roleArn": role,
            }),
        )
        .await;
    let expected = json!({
        "DestinationName": native,
        "Arn": engine::emulator_arn(
            "logs",
            super::emulator::SIGNING_REGION,
            &format!("destination:{native}"),
        ),
        "TargetArn": target,
        "RoleArn": role,
    });
    let reported = emulator.properties(DESTINATION, native).await;
    assert_eq!(
        reported, expected,
        "a destination the owning service holds must resolve as {expected:?}, got \
         {reported:?}"
    );

    // Created through Cloud Control API, reported by the owning service's own reader.
    let through_cc = "cloud-control-destination";
    assert_eq!(
        emulator
            .provision(
                DESTINATION,
                &json!({
                    "DestinationName": through_cc,
                    "TargetArn": target,
                    "RoleArn": role,
                }),
            )
            .await,
        through_cc
    );
    let described = emulator.logs_ok("DescribeDestinations", json!({})).await;
    let held: Vec<String> = described["destinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|destination| text(destination, "destinationName"))
        .collect();
    assert_eq!(
        held,
        vec![through_cc.to_string(), native.to_string()],
        "the owning service must hold both destinations, it holds {held:?}"
    );
    assert_eq!(
        emulator.walk(DESTINATION, None).await,
        vec![through_cc.to_string(), native.to_string()],
        "the listing must report the owning service's inventory"
    );

    // Deleted natively, gone from Cloud Control API too.
    emulator
        .logs_ok("DeleteDestination", json!({ "destinationName": native }))
        .await;
    let (status, body) = emulator.get(DESTINATION, native).await;
    assert_eq!(
        status, 404,
        "GetResource after the native delete: expected HTTP 404, got {status} with body \
         {body:?}"
    );
    assert_handler_not_found(
        &message(&body),
        DESTINATION,
        &not_found_detail(DESTINATION, native),
        "GetResource after the native delete",
    );

    // Deleted through Cloud Control API, gone from the owning service too.
    emulator.delete_and_settle(DESTINATION, through_cc).await;
    let described = emulator.logs_ok("DescribeDestinations", json!({})).await;
    assert_eq!(
        described["destinations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "the owning service must hold no destination, got {described:?}"
    );

    // A model the owning service will not take is refused rather than half-applied: a
    // destination IS the pair of ARNs it parses together.
    for (case, desired) in [
        (
            "a model naming no target",
            json!({ "DestinationName": "incomplete", "RoleArn": role }),
        ),
        (
            "a target that is not an ARN",
            json!({ "DestinationName": "incomplete", "TargetArn": "feed", "RoleArn": role }),
        ),
    ] {
        let accepted = emulator.create_ok(DESTINATION, &desired, None).await;
        let settled = emulator.terminal(&text(&accepted, "RequestToken")).await;
        assert_eq!(
            text(&settled, "OperationStatus"),
            STATUS_FAILED,
            "case {case:?}: expected the request to fail, got {settled:?}"
        );
        assert_eq!(
            text(&settled, "ErrorCode"),
            "InvalidRequest",
            "case {case:?}: unexpected handler error code, got {settled:?}"
        );
        let (status, body) = emulator.get(DESTINATION, "incomplete").await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must leave no resource, got {status} with body \
             {body:?}"
        );
    }
}
