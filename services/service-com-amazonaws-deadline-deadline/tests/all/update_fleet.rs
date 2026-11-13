// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateFleet`.
//!
//! The rules under test:
//!
//! * `UpdateFleet` is a PATCH of the fleet its path names: every member the request carries
//!   is applied and every member it leaves out keeps the value the fleet holds, which a
//!   `GetFleet` of the same fleet reports back;
//! * the `configuration` union is replaced by the one the request carries, whichever member
//!   it uses, and the capabilities the fleet reports follow it;
//! * a fleet is reached only through its own farm, so an update naming an unheld farm, an
//!   unheld fleet, a fleet of another farm or a fleet that was just deleted changes nothing
//!   and answers HTTP 404 `ResourceNotFoundException` whose message is AWS's one sentence,
//!   `Resource of type <resourceType> with id <resourceId> does not exist.`, with the kind
//!   the path addresses and the id the request named;
//! * the worker-count window the update would LEAVE the fleet with has to be a window: a
//!   minimum above the maximum — whichever of the two the request carries — is refused with
//!   `ValidationException` and applies nothing, and so is an inverted capability range;
//! * `clientToken` is `@idempotencyToken`, so an update retried with the same token on the
//!   same fleet answers the first call's success and writes nothing a second time.
//!
//! The harness (`Emulator`) and the fleet helpers are the ones the `fleet` suite drives the
//! service with, so these tests go through routing, deserialization, constraint validation,
//! the handler and serialization, exactly as the router does in production.

use serde_json::{json, Map, Value};

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};
use crate::fleet::{
    at, create_fleet, create_fleet_path, customer_managed, get_fleet_path, service_managed,
    ROLE_ARN,
};
use crate::resource_not_found_wording::not_found_message;

/// A second IAM role ARN satisfying the modeled `IamRoleArn` pattern, so a row can move
/// `roleArn` to a different value of the same shape.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/DeadlineFleetUpdated";

/// The fleet members an `UpdateFleet` request can move
/// (`smithy_query(query="operation", operation="UpdateFleet")`), as `GetFleet` reports them.
const UPDATABLE: [&str; 7] = [
    "displayName",
    "description",
    "roleArn",
    "minWorkerCount",
    "maxWorkerCount",
    "configuration",
    "hostConfiguration",
];

/// Those members of one `GetFleet` body, with the ones the response leaves out left out, so
/// two reads can be compared member by member.
fn updatable(fleet: &Value) -> Value {
    let mut members = Map::new();
    for member in UPDATABLE {
        if let Some(value) = fleet.get(member) {
            members.insert(member.to_string(), value.clone());
        }
    }
    Value::Object(members)
}

/// `PATCH /2023-10-12/farms/{farmId}/fleets/{fleetId}`, with an optional client token.
async fn update_fleet(
    emulator: &Emulator,
    farm_id: &str,
    fleet_id: &str,
    body: Value,
    client_token: Option<&str>,
) -> (u16, Value) {
    emulator
        .call(
            "PATCH",
            &get_fleet_path(farm_id, fleet_id),
            Some(body),
            client_token,
        )
        .await
}

/// `GET /2023-10-12/farms/{farmId}/fleets/{fleetId}`, failing the test if it is not served.
async fn read_fleet(emulator: &Emulator, farm_id: &str, fleet_id: &str, case: &str) -> Value {
    let (status, fleet) = emulator.get(&get_fleet_path(farm_id, fleet_id)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetFleet to answer 200, got {status} with {fleet}"
    );
    fleet
}

/// Create the fleet the update rows move, with every member an update can move already set,
/// so each row is shown to CHANGE a value rather than to fill in a missing one.
async fn create_full_fleet(emulator: &Emulator, farm_id: &str) -> String {
    let (status, body) = emulator
        .post(
            &create_fleet_path(farm_id),
            json!({
                "displayName": "before any update",
                "description": "the description the create stored",
                "roleArn": ROLE_ARN,
                "minWorkerCount": 1,
                "maxWorkerCount": 4,
                "configuration": customer_managed("LINUX", "x86_64", json!({})),
                "hostConfiguration": {
                    "scriptBody": "echo created",
                    "scriptTimeoutSeconds": 300,
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateFleet: expected 200, got {status} with {body}"
    );
    string_at(&body, "fleetId")
}

/// One row: the request body, and the members it must leave the fleet reporting differently
/// (`None` for a member the response must then leave out).
struct Applied {
    case: &'static str,
    patch: Value,
    changes: Vec<(&'static str, Option<Value>)>,
}

/// One row of the worker-count window table: the bounds the request carries (`None` for a
/// bound it leaves to the fleet) and, when the window the update would leave is not a
/// window, the pair the refusal must name.
struct Window {
    case: &'static str,
    min: Option<i64>,
    max: Option<i64>,
    refused: Option<(i64, i64)>,
}

/// One row of the capability-range table: the pair's own path in the request, the
/// configuration declaring it and, when that pair is inverted, its bounds as the refusal
/// words them.
struct Range {
    case: &'static str,
    member: String,
    configuration: Value,
    refused: Option<(&'static str, &'static str)>,
}

#[tokio::test]
async fn update_fleet_applies_the_members_it_carries_and_leaves_the_rest() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("updating").await;
    let fleet_id = create_full_fleet(&emulator, &farm_id).await;
    // A second fleet of the same farm, so every row is also shown to move ONE fleet.
    let bystander = create_fleet(&emulator, &farm_id, "untouched by every update").await;
    let bystander_before = read_fleet(&emulator, &farm_id, &bystander, "the bystander").await;

    // The rule: the request's members are applied, the rest keep what they hold. The rows
    // differ along what the update carries — a string, an integer, an ARN, a nested
    // structure, several members at once, nothing at all, and a member set to the empty
    // string the model allows it — and they run against one fleet in order, so each row also
    // reads back what the row before it wrote.
    let rows: Vec<Applied> = vec![
        Applied {
            case: "one string member",
            patch: json!({"displayName": "renamed once"}),
            changes: vec![("displayName", Some(json!("renamed once")))],
        },
        Applied {
            case: "another string member, at the modelled maximum length",
            patch: json!({"description": "d".repeat(100)}),
            changes: vec![("description", Some(json!("d".repeat(100))))],
        },
        Applied {
            case: "an integer member: the window's ceiling",
            patch: json!({"maxWorkerCount": 9}),
            changes: vec![("maxWorkerCount", Some(json!(9)))],
        },
        Applied {
            case: "the other integer member: the window's floor",
            patch: json!({"minWorkerCount": 3}),
            changes: vec![("minWorkerCount", Some(json!(3)))],
        },
        Applied {
            case: "an ARN member",
            patch: json!({"roleArn": OTHER_ROLE_ARN}),
            changes: vec![("roleArn", Some(json!(OTHER_ROLE_ARN)))],
        },
        Applied {
            case: "a nested structure",
            patch: json!({
                "hostConfiguration": {"scriptBody": "echo updated", "scriptTimeoutSeconds": 900},
            }),
            changes: vec![(
                "hostConfiguration",
                Some(json!({"scriptBody": "echo updated", "scriptTimeoutSeconds": 900})),
            )],
        },
        Applied {
            case: "four members at once",
            patch: json!({
                "displayName": "renamed again",
                "description": "a shorter description",
                "minWorkerCount": 0,
                "maxWorkerCount": 2147483647,
            }),
            changes: vec![
                ("displayName", Some(json!("renamed again"))),
                ("description", Some(json!("a shorter description"))),
                ("minWorkerCount", Some(json!(0))),
                ("maxWorkerCount", Some(json!(2147483647i64))),
            ],
        },
        Applied {
            case: "a member set to the value it already holds",
            patch: json!({"displayName": "renamed again"}),
            changes: vec![],
        },
        Applied {
            case: "a request that carries no member at all",
            patch: json!({}),
            changes: vec![],
        },
        Applied {
            // `Description` is `len 0..100`, so the empty string is a value the member can
            // take; an empty description is nothing to report, so the response leaves it out.
            case: "the empty description the model allows",
            patch: json!({"description": ""}),
            changes: vec![("description", None)],
        },
        Applied {
            // `smithy_query(query="shape", shape="HostConfiguration")` documents this as the
            // way a script is taken off a fleet.
            case: "an empty scriptBody, which takes the script off the fleet",
            patch: json!({"hostConfiguration": {"scriptBody": "", "scriptTimeoutSeconds": 300}}),
            changes: vec![("hostConfiguration", None)],
        },
    ];

    for row in rows {
        let before = read_fleet(&emulator, &farm_id, &fleet_id, row.case).await;
        let mut expected = updatable(&before);
        {
            let members = expected
                .as_object_mut()
                .expect("the tracked members are an object");
            for (member, value) in &row.changes {
                match value {
                    Some(value) => {
                        members.insert((*member).to_string(), value.clone());
                    }
                    None => {
                        members.remove(*member);
                    }
                }
            }
        }

        let (status, body) = update_fleet(&emulator, &farm_id, &fleet_id, row.patch.clone(), None)
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected UpdateFleet to answer 200, got {status} with {body}",
            row.case
        );
        assert!(
            body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
            "case {:?}: expected an empty UpdateFleet response, got {body}",
            row.case
        );

        let after = read_fleet(&emulator, &farm_id, &fleet_id, row.case).await;
        assert_eq!(
            updatable(&after),
            expected,
            "case {:?}: expected the update to apply {:?} and leave the rest, got {after}",
            row.case,
            row.changes
        );
        // The fleet is still the same fleet, under the same farm.
        assert_eq!(
            (string_at(&after, "fleetId"), string_at(&after, "farmId")),
            (fleet_id.clone(), farm_id.clone()),
            "case {:?}: expected the update to leave the fleet's identity alone, got {after}",
            row.case
        );
        // An updated fleet reports who updated it and when, whatever the row carried: the
        // call itself is the update.
        assert!(
            after.get("updatedAt").and_then(Value::as_str).is_some()
                && !string_at(&after, "updatedBy").is_empty(),
            "case {:?}: expected updatedAt and updatedBy on an updated fleet, got {after}",
            row.case
        );
        // A read does not change what the next reader sees.
        let again = read_fleet(&emulator, &farm_id, &fleet_id, row.case).await;
        assert_eq!(
            updatable(&again),
            updatable(&after),
            "case {:?}: two reads of one updated fleet disagreed: {after} then {again}",
            row.case
        );
    }

    // No row of the table reached the other fleet of this farm.
    let bystander_after = read_fleet(&emulator, &farm_id, &bystander, "the bystander").await;
    assert_eq!(
        updatable(&bystander_after),
        updatable(&bystander_before),
        "expected the other fleet of this farm to be untouched, got {bystander_after}"
    );
    assert!(
        bystander_after.get("updatedAt").is_none(),
        "expected no updatedAt on a fleet nothing updated, got {bystander_after}"
    );
}

#[tokio::test]
async fn update_fleet_replaces_the_fleet_configuration() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("reconfiguring").await;
    let fleet_id = create_full_fleet(&emulator, &farm_id).await;

    // The rule: `configuration` is a union, and the update replaces the fleet's with the one
    // the request carries. The rows swap the member in both directions, and carry the custom
    // capabilities a configuration can name as well as none at all; the fleet's reported
    // `capabilities` follow the configuration it now holds.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "customer managed, to the other union member",
            "serviceManagedEc2.instanceCapabilities",
            service_managed("WINDOWS", "x86_64", "spot", json!({})),
        ),
        (
            "service managed, with custom capabilities",
            "serviceManagedEc2.instanceCapabilities",
            service_managed(
                "LINUX",
                "arm64",
                "on-demand",
                json!({
                    "customAmounts": [{"name": "amount.custom.licenses", "min": 2.0, "max": 8.0}],
                    "customAttributes": [{"name": "attr.custom.gpu", "values": ["a10g"]}],
                }),
            ),
        ),
        (
            "service managed, back to customer managed",
            "customerManaged.workerCapabilities",
            customer_managed("MACOS", "arm64", json!({})),
        ),
        (
            "customer managed, with custom capabilities",
            "customerManaged.workerCapabilities",
            customer_managed(
                "LINUX",
                "x86_64",
                json!({
                    "customAmounts": [{"name": "amount.custom.pixels", "min": 1.0}],
                    "customAttributes": [
                        {"name": "attr.custom.renderer", "values": ["blender", "maya"]},
                    ],
                }),
            ),
        ),
    ];

    for (case, capabilities, configuration) in rows {
        let before = read_fleet(&emulator, &farm_id, &fleet_id, case).await;
        let (status, body) = update_fleet(
            &emulator,
            &farm_id,
            &fleet_id,
            json!({"configuration": configuration}),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateFleet to answer 200, got {status} with {body}"
        );

        let after = read_fleet(&emulator, &farm_id, &fleet_id, case).await;
        let reported = after
            .get("configuration")
            .expect("configuration is required in the response");
        // The configuration the fleet now holds is the one the request sent: the union member
        // it used, with the capability leaves it declared.
        for leaf in [
            "osFamily",
            "cpuArchitectureType",
            "vCpuCount.min",
            "memoryMiB.min",
            "customAmounts",
            "customAttributes",
        ] {
            let path = format!("{capabilities}.{leaf}");
            assert_eq!(
                at(reported, &path),
                at(&configuration, &path),
                "case {case:?}: expected configuration {path} to be the one the update sent, \
                 got {after}"
            );
        }
        // Only one member of the union is reported: the other is gone with the configuration
        // it belonged to.
        let replaced = if capabilities.starts_with("customerManaged") {
            "serviceManagedEc2"
        } else {
            "customerManaged"
        };
        assert!(
            reported.get(replaced).is_none(),
            "case {case:?}: expected the replaced union member {replaced} to be gone, got {after}"
        );
        // `capabilities` reports what the configuration NAMES, so it follows the replacement.
        assert_eq!(
            (
                after
                    .get("capabilities")
                    .and_then(|capabilities| capabilities.get("amounts")),
                after
                    .get("capabilities")
                    .and_then(|capabilities| capabilities.get("attributes")),
            ),
            (
                at(&configuration, &format!("{capabilities}.customAmounts")),
                at(&configuration, &format!("{capabilities}.customAttributes")),
            ),
            "case {case:?}: expected the reported capabilities to follow the configuration, \
             got {after}"
        );
        // The members this request did not carry are untouched by the replacement.
        assert_eq!(
            (
                string_at(&after, "displayName"),
                string_at(&after, "roleArn"),
                after.get("minWorkerCount").and_then(Value::as_i64),
                after.get("maxWorkerCount").and_then(Value::as_i64),
            ),
            (
                string_at(&before, "displayName"),
                string_at(&before, "roleArn"),
                before.get("minWorkerCount").and_then(Value::as_i64),
                before.get("maxWorkerCount").and_then(Value::as_i64),
            ),
            "case {case:?}: expected a configuration replacement to leave the other members \
             alone, got {after}"
        );
    }
}

#[tokio::test]
async fn update_fleet_refuses_a_fleet_it_does_not_hold() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("owner").await;
    let other_farm_id = emulator.farm("stranger").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "owned").await;
    let deleted_id = create_fleet(&emulator, &farm_id, "deleted before the update").await;
    let (status, body) = emulator
        .call(
            "DELETE",
            &get_fleet_path(&farm_id, &deleted_id),
            None,
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "DeleteFleet: expected 200, got {status} with {body}"
    );

    // Every id below is well formed for its modelled `^<prefix>-[0-9a-f]{32}$` label pattern,
    // so the boundary validator passes the request and the handler is what decides the
    // outcome. The rows differ along WHICH level of the path is missing and how it came to be
    // missing — never held, held by another farm, or deleted — and along the id's digits.
    let absent_farm = absent_id("farm", &format!("{:032x}", 0x11));
    let absent_fleet = absent_id("fleet", &format!("{:032x}", 0x12));
    let all_f_fleet = absent_id("fleet", &"f".repeat(32));
    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "a fleet that was just deleted",
            farm_id.clone(),
            deleted_id.clone(),
            "fleet",
            deleted_id.clone(),
        ),
        (
            "a fleet of another farm",
            other_farm_id.clone(),
            fleet_id.clone(),
            "fleet",
            fleet_id.clone(),
        ),
        (
            "a fleet nothing ever held",
            farm_id.clone(),
            absent_fleet.clone(),
            "fleet",
            absent_fleet.clone(),
        ),
        (
            "a fleet id of all f",
            farm_id.clone(),
            all_f_fleet.clone(),
            "fleet",
            all_f_fleet.clone(),
        ),
        (
            "an unheld farm",
            absent_farm.clone(),
            fleet_id.clone(),
            "farm",
            absent_farm.clone(),
        ),
        (
            "an unheld farm and an unheld fleet: the farm is the level that is missing",
            absent_farm.clone(),
            absent_fleet.clone(),
            "farm",
            absent_farm.clone(),
        ),
    ];

    for (case, farm, fleet, resource_type, resource_id) in cases {
        let (status, body) = update_fleet(
            &emulator,
            &farm,
            &fleet,
            json!({"displayName": "anything"}),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected UpdateFleet to answer 404, got {status} with {body}"
        );
        assert_eq!(
            (
                error_code(&body),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId"),
                string_at(&body, "message"),
            ),
            (
                "ResourceNotFoundException".to_string(),
                resource_type.to_string(),
                resource_id.clone(),
                not_found_message(resource_type, &resource_id),
            ),
            "case {case:?}: expected the 404 to name the missing kind and id in AWS's sentence, \
             got {body}"
        );
    }

    // None of those refusals changed anything: the held fleet still carries the name its
    // create gave it, and nothing marked it updated.
    let fleet = read_fleet(&emulator, &farm_id, &fleet_id, "after the refusals").await;
    assert_eq!(
        (string_at(&fleet, "displayName"), fleet.get("updatedAt")),
        ("owned".to_string(), None),
        "after the refusals: expected the held fleet to be untouched, got {fleet}"
    );
}

#[tokio::test]
async fn update_fleet_refuses_a_window_that_is_not_a_window() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("windows").await;

    // The rule: the fleet's minimum worker count is the floor it is scaled to and its maximum
    // the ceiling it never passes, so the window the update would LEAVE the fleet with — each
    // bound the request omits taken from the fleet — is refused when the minimum is above the
    // maximum. The rows differ along which of the two the request carries, both published
    // bounds of `MinZeroMaxInteger` (`range 0..2147483647`) and both sides of equality. The
    // fleet each row runs against is created with the window 2..6.
    let stored_min = 2;
    let stored_max = 6;
    let cases: Vec<Window> = vec![
        Window { case: "neither bound", min: None, max: None, refused: None },
        Window {
            case: "a minimum below the stored maximum",
            min: Some(0),
            max: None,
            refused: None,
        },
        Window {
            case: "a minimum equal to the stored maximum",
            min: Some(6),
            max: None,
            refused: None,
        },
        Window {
            case: "a maximum equal to the stored minimum",
            min: None,
            max: Some(2),
            refused: None,
        },
        Window {
            case: "both bounds, in order",
            min: Some(1),
            max: Some(3),
            refused: None,
        },
        Window {
            case: "both bounds equal",
            min: Some(7),
            max: Some(7),
            refused: None,
        },
        Window {
            case: "both bounds at the published maximum",
            min: Some(2147483647),
            max: Some(2147483647),
            refused: None,
        },
        Window {
            case: "a minimum one above the stored maximum",
            min: Some(7),
            max: None,
            refused: Some((7, stored_max)),
        },
        Window {
            case: "the published maximum as a minimum, against the stored maximum",
            min: Some(2147483647),
            max: None,
            refused: Some((2147483647, stored_max)),
        },
        Window {
            case: "a maximum one below the stored minimum",
            min: None,
            max: Some(1),
            refused: Some((stored_min, 1)),
        },
        Window {
            case: "a zero maximum, against the stored minimum",
            min: None,
            max: Some(0),
            refused: Some((stored_min, 0)),
        },
        Window {
            case: "both bounds, inverted",
            min: Some(5),
            max: Some(4),
            refused: Some((5, 4)),
        },
        Window {
            case: "both bounds, inverted at the published bounds",
            min: Some(2147483647),
            max: Some(0),
            refused: Some((2147483647, 0)),
        },
    ];

    for Window {
        case,
        min,
        max,
        refused,
    } in cases
    {
        // One fleet per row, so each row is judged against the same stored window.
        let (status, created) = emulator
            .post(
                &create_fleet_path(&farm_id),
                json!({
                    "displayName": "windowed",
                    "roleArn": ROLE_ARN,
                    "minWorkerCount": stored_min,
                    "maxWorkerCount": stored_max,
                    "configuration": customer_managed("LINUX", "x86_64", json!({})),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateFleet to answer 200, got {status} with {created}"
        );
        let fleet_id = string_at(&created, "fleetId");

        let mut patch = json!({"displayName": "renamed by the same request"});
        if let Some(min) = min {
            patch["minWorkerCount"] = json!(min);
        }
        if let Some(max) = max {
            patch["maxWorkerCount"] = json!(max);
        }
        let (status, body) = update_fleet(&emulator, &farm_id, &fleet_id, patch, None).await;

        let Some((refused_min, refused_max)) = refused else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateFleet to answer 200, got {status} with {body}"
            );
            let fleet = read_fleet(&emulator, &farm_id, &fleet_id, case).await;
            assert_eq!(
                (
                    fleet.get("minWorkerCount").and_then(Value::as_i64),
                    fleet.get("maxWorkerCount").and_then(Value::as_i64),
                ),
                (
                    Some(min.unwrap_or(stored_min)),
                    Some(max.unwrap_or(stored_max)),
                ),
                "case {case:?}: expected the accepted window to be stored, got {fleet}"
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {case:?}: expected UpdateFleet to answer 400, got {status} with {body}"
        );
        assert_eq!(
            (error_code(&body), string_at(&body, "reason")),
            (
                "ValidationException".to_string(),
                "FIELD_VALIDATION_FAILED".to_string()
            ),
            "case {case:?}: expected a field-validation refusal, got {body}"
        );
        let field_names: Vec<String> = body
            .get("fieldList")
            .and_then(Value::as_array)
            .map(|fields| fields.iter().map(|field| string_at(field, "name")).collect())
            .unwrap_or_default();
        assert_eq!(
            field_names,
            vec!["minWorkerCount".to_string()],
            "case {case:?}: expected the refusal to name minWorkerCount, got {body}"
        );
        // The message names the window the fleet would have been left with, so its variable
        // parts come from the request and the fleet rather than being fixed in the handler.
        let expected_message = format!(
            "minWorkerCount ({refused_min}) must be less than or equal to \
             maxWorkerCount ({refused_max})"
        );
        assert_eq!(
            string_at(&body, "message"),
            expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {body}"
        );

        // The refusal applied nothing at all — not the window, and not the other member the
        // same request asked to change.
        let fleet = read_fleet(&emulator, &farm_id, &fleet_id, case).await;
        assert_eq!(
            (
                fleet.get("minWorkerCount").and_then(Value::as_i64),
                fleet.get("maxWorkerCount").and_then(Value::as_i64),
                string_at(&fleet, "displayName"),
                fleet.get("updatedAt").cloned(),
            ),
            (
                Some(stored_min),
                Some(stored_max),
                "windowed".to_string(),
                None
            ),
            "case {case:?}: expected the refused update to apply nothing, got {fleet}"
        );
    }
}

#[tokio::test]
async fn update_fleet_refuses_an_inverted_capability_range() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("ranges").await;
    let fleet_id = create_full_fleet(&emulator, &farm_id).await;

    // Where the request nests a fleet's capabilities, per the configuration union member: a
    // refusal names the pair's own path in the request.
    const CUSTOMER_CAPS: &str = "configuration.customerManaged.workerCapabilities";
    const SERVICE_CAPS: &str = "configuration.serviceManagedEc2.instanceCapabilities";

    // The same window rule applied to the other lower/upper pairs a fleet request carries:
    // the capability ranges its configuration declares. The rows walk the ranges the model
    // defines, on both union members, with the pair inverted, in order and open-ended. A
    // refused row carries the pair's bounds as the message words them.
    let cases: Vec<Range> = vec![
        Range {
            case: "a customer managed vCPU range in order",
            member: format!("{CUSTOMER_CAPS}.vCpuCount"),
            configuration: customer_managed(
                "LINUX",
                "x86_64",
                json!({"vCpuCount": {"min": 1, "max": 8}}),
            ),
            refused: None,
        },
        Range {
            case: "a customer managed vCPU range with no upper bound",
            member: format!("{CUSTOMER_CAPS}.vCpuCount"),
            configuration: customer_managed("LINUX", "x86_64", json!({"vCpuCount": {"min": 64}})),
            refused: None,
        },
        Range {
            case: "a customer managed vCPU range with equal bounds",
            member: format!("{CUSTOMER_CAPS}.vCpuCount"),
            configuration: customer_managed(
                "LINUX",
                "x86_64",
                json!({"vCpuCount": {"min": 4, "max": 4}}),
            ),
            refused: None,
        },
        Range {
            case: "an inverted customer managed vCPU range",
            member: format!("{CUSTOMER_CAPS}.vCpuCount"),
            configuration: customer_managed(
                "LINUX",
                "x86_64",
                json!({"vCpuCount": {"min": 8, "max": 2}}),
            ),
            refused: Some(("8", "2")),
        },
        Range {
            case: "an inverted customer managed memory range",
            member: format!("{CUSTOMER_CAPS}.memoryMiB"),
            configuration: customer_managed(
                "LINUX",
                "x86_64",
                json!({"memoryMiB": {"min": 4096, "max": 1024}}),
            ),
            refused: Some(("4096", "1024")),
        },
        Range {
            case: "an inverted customer managed accelerator count",
            member: format!("{CUSTOMER_CAPS}.acceleratorCount"),
            configuration: customer_managed(
                "LINUX",
                "x86_64",
                json!({"acceleratorCount": {"min": 4, "max": 1}}),
            ),
            refused: Some(("4", "1")),
        },
        Range {
            case: "an inverted customer managed accelerator memory range",
            member: format!("{CUSTOMER_CAPS}.acceleratorTotalMemoryMiB"),
            configuration: customer_managed(
                "LINUX",
                "x86_64",
                json!({"acceleratorTotalMemoryMiB": {"min": 2048, "max": 512}}),
            ),
            refused: Some(("2048", "512")),
        },
        Range {
            case: "an inverted customer managed custom amount, named by its entry in the request",
            member: format!("{CUSTOMER_CAPS}.customAmounts[0]"),
            configuration: customer_managed(
                "LINUX",
                "x86_64",
                json!({"customAmounts": [{"name": "amount.custom.pixels", "min": 4.0, "max": 1.0}]}),
            ),
            refused: Some(("4", "1")),
        },
        Range {
            case: "an inverted service managed memory range",
            member: format!("{SERVICE_CAPS}.memoryMiB"),
            configuration: service_managed(
                "WINDOWS",
                "x86_64",
                "spot",
                json!({"memoryMiB": {"min": 8192, "max": 2048}}),
            ),
            refused: Some(("8192", "2048")),
        },
        Range {
            case: "an inverted service managed accelerator count",
            member: format!("{SERVICE_CAPS}.acceleratorCount"),
            configuration: service_managed(
                "LINUX",
                "x86_64",
                "spot",
                json!({
                    "acceleratorCapabilities": {
                        "selections": [{"name": "t4"}],
                        "count": {"min": 2, "max": 1},
                    },
                }),
            ),
            refused: Some(("2", "1")),
        },
    ];

    for Range {
        case,
        member,
        configuration,
        refused,
    } in cases
    {
        let before = read_fleet(&emulator, &farm_id, &fleet_id, case).await;
        let (status, body) = update_fleet(
            &emulator,
            &farm_id,
            &fleet_id,
            json!({"configuration": configuration}),
            None,
        )
        .await;

        let Some((min, max)) = refused else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateFleet to answer 200, got {status} with {body}"
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {case:?}: expected UpdateFleet to answer 400, got {status} with {body}"
        );
        let field_names: Vec<String> = body
            .get("fieldList")
            .and_then(Value::as_array)
            .map(|fields| fields.iter().map(|field| string_at(field, "name")).collect())
            .unwrap_or_default();
        assert_eq!(
            field_names,
            vec![member.clone()],
            "case {case:?}: expected the refusal to name {member}, got {body}"
        );
        let expected_message = format!(
            "{member}.min ({min}) must be less than or equal to {member}.max ({max})"
        );
        assert_eq!(
            (error_code(&body), string_at(&body, "message")),
            ("ValidationException".to_string(), expected_message.clone()),
            "case {case:?}: expected the refusal {expected_message:?}, got {body}"
        );

        // The refusal replaced nothing: the fleet still holds the configuration it held.
        let after = read_fleet(&emulator, &farm_id, &fleet_id, case).await;
        assert_eq!(
            after.get("configuration"),
            before.get("configuration"),
            "case {case:?}: expected the fleet to keep the configuration it held, got {after}"
        );
    }
}

#[tokio::test]
async fn one_client_token_updates_one_fleet_once() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("idempotent updates").await;
    let fleet_id = create_full_fleet(&emulator, &farm_id).await;
    let other_id = create_fleet(&emulator, &farm_id, "the other fleet").await;

    let (status, body) = update_fleet(
        &emulator,
        &farm_id,
        &fleet_id,
        json!({"displayName": "named by the first call"}),
        Some("update-one"),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first update: expected 200, got {status} with {body}"
    );
    let first = read_fleet(&emulator, &farm_id, &fleet_id, "the first update").await;

    // The rule: `clientToken` lets the server recognize a retry of the SAME request, so a
    // call naming that token AND that fleet answers the first call's success without writing
    // again, while any other call is an update of its own. The rows differ along what changes
    // between the two calls: nothing, the token, or the fleet the update names.
    let cases: Vec<(&str, Option<&str>, &str, bool)> = vec![
        (
            "the same token on the same fleet",
            Some("update-one"),
            "renamed by a retry",
            false,
        ),
        (
            "the same token again",
            Some("update-one"),
            "renamed by another retry",
            false,
        ),
        (
            "another token on the same fleet",
            Some("update-two"),
            "renamed by a new token",
            true,
        ),
        (
            "no token on the same fleet",
            None,
            "renamed with no token",
            true,
        ),
    ];

    for (case, token, display_name, applied) in cases {
        let (status, body) = update_fleet(
            &emulator,
            &farm_id,
            &fleet_id,
            json!({"displayName": display_name}),
            token,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateFleet to answer 200, got {status} with {body}"
        );

        let fleet = read_fleet(&emulator, &farm_id, &fleet_id, case).await;
        let expected_name = if applied {
            display_name.to_string()
        } else {
            string_at(&first, "displayName")
        };
        assert_eq!(
            string_at(&fleet, "displayName"),
            expected_name,
            "case {case:?}: expected the fleet to be named {expected_name:?}, got {fleet}"
        );
        if !applied {
            // A recognized retry writes nothing again, so the fleet keeps the `updatedAt` of
            // the call the caller actually made.
            assert_eq!(
                fleet.get("updatedAt"),
                first.get("updatedAt"),
                "case {case:?}: expected a recognized retry to leave updatedAt alone, got {fleet}"
            );
        }
    }

    // The token is scoped to the fleet it was used on: the same token on another fleet is an
    // update of that fleet, not a retry of this one.
    let before_sharing = read_fleet(&emulator, &farm_id, &fleet_id, "the shared token").await;
    let (status, body) = update_fleet(
        &emulator,
        &farm_id,
        &other_id,
        json!({"displayName": "named through the shared token"}),
        Some("update-one"),
    )
    .await;
    assert_eq!(
        status, 200,
        "the shared token on another fleet: expected 200, got {status} with {body}"
    );
    let other = read_fleet(&emulator, &farm_id, &other_id, "the shared token").await;
    assert_eq!(
        string_at(&other, "displayName"),
        "named through the shared token",
        "expected the other fleet to be updated through the shared token, got {other}"
    );

    // Spending that token on the other fleet did not cost the first fleet its own retry
    // record: a retry of the first call is still recognized and still writes nothing.
    let (status, body) = update_fleet(
        &emulator,
        &farm_id,
        &fleet_id,
        json!({"displayName": "renamed by a late retry"}),
        Some("update-one"),
    )
    .await;
    assert_eq!(
        status, 200,
        "the late retry: expected 200, got {status} with {body}"
    );
    let late = read_fleet(&emulator, &farm_id, &fleet_id, "the late retry").await;
    assert_eq!(
        (
            string_at(&late, "displayName"),
            late.get("updatedAt").cloned()
        ),
        (
            string_at(&before_sharing, "displayName"),
            before_sharing.get("updatedAt").cloned()
        ),
        "after the shared token: expected the late retry to write nothing, got {late}"
    );
}
