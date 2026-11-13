// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the fleet slice of AWS Deadline Cloud: `CreateFleet`,
//! `GetFleet` and `ListFleets`.
//!
//! The rules under test:
//!
//! * a fleet is created under its farm and `GetFleet` reports the values that create
//!   persisted, under a minted `^fleet-[0-9a-f]{32}$` id;
//! * a fleet's worker-count window is a window: a request whose `minWorkerCount` exceeds
//!   its `maxWorkerCount` is refused with `ValidationException` and creates nothing;
//! * a new fleet is `CREATE_IN_PROGRESS` and reaches `ACTIVE` when the clock makes that
//!   move due, not when something reads it;
//! * `clientToken` deduplicates a retried create;
//! * a fleet is reachable only through its own farm;
//! * `ListFleets` reports the fleets of one farm, oldest first, one page at a time, and
//!   only those matching the filters the request carries.
//!
//! The harness (`Emulator`) is the one `farm_queue_job` already drives the service with,
//! so these tests go through routing, deserialization, constraint validation, the handler
//! and serialization, exactly as the router does in production.

use std::time::Duration;

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, is_resource_id, string_at, Emulator};

/// The value at a dotted member path, e.g. `customerManaged.workerCapabilities.osFamily`.
///
/// `pub(crate)` so every suite in this test target walks a response the same way.
pub(crate) fn at<'a>(body: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cursor = body;
    for member in path.split('.') {
        cursor = cursor.get(member)?;
    }
    Some(cursor)
}

/// A customer managed fleet configuration, the smallest the model accepts: `mode`
/// (`AutoScalingMode`) and `workerCapabilities` are both required, and that structure
/// requires `vCpuCount`, `memoryMiB`, `osFamily` and `cpuArchitectureType`
/// (`smithy_query(query="closure", operation="CreateFleet")`).
pub(crate) fn customer_managed(os_family: &str, cpu: &str, extra: Value) -> Value {
    let mut capabilities = json!({
        "vCpuCount": {"min": 1, "max": 4},
        "memoryMiB": {"min": 512, "max": 4096},
        "osFamily": os_family,
        "cpuArchitectureType": cpu,
    });
    if let (Some(capabilities), Some(extra)) = (capabilities.as_object_mut(), extra.as_object()) {
        for (member, value) in extra {
            capabilities.insert(member.clone(), value.clone());
        }
    }
    json!({"customerManaged": {"mode": "NO_SCALING", "workerCapabilities": capabilities}})
}

/// A service managed Amazon EC2 fleet configuration: `instanceCapabilities` and
/// `instanceMarketOptions` are both required.
pub(crate) fn service_managed(os_family: &str, cpu: &str, market: &str, extra: Value) -> Value {
    let mut capabilities = json!({
        "vCpuCount": {"min": 2},
        "memoryMiB": {"min": 1024},
        "osFamily": os_family,
        "cpuArchitectureType": cpu,
    });
    if let (Some(capabilities), Some(extra)) = (capabilities.as_object_mut(), extra.as_object()) {
        for (member, value) in extra {
            capabilities.insert(member.clone(), value.clone());
        }
    }
    json!({
        "serviceManagedEc2": {
            "instanceCapabilities": capabilities,
            "instanceMarketOptions": {"type": market},
        }
    })
}

/// Where a configuration's worker capabilities sit, per the union member the request used.
fn capabilities_path(configuration: &Value) -> &'static str {
    if configuration.get("customerManaged").is_some() {
        "customerManaged.workerCapabilities"
    } else {
        "serviceManagedEc2.instanceCapabilities"
    }
}

pub(crate) fn create_fleet_path(farm_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/fleets")
}

pub(crate) fn get_fleet_path(farm_id: &str, fleet_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/fleets/{fleet_id}")
}

/// An IAM role ARN satisfying the modeled `IamRoleArn` pattern.
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/DeadlineFleet";

/// Create a fleet under `farm_id` with `display_name` and return its id.
pub(crate) async fn create_fleet(emulator: &Emulator, farm_id: &str, display_name: &str) -> String {
    let (status, body) = emulator
        .post(
            &create_fleet_path(farm_id),
            json!({
                "displayName": display_name,
                "roleArn": ROLE_ARN,
                "maxWorkerCount": 4,
                "configuration": customer_managed("LINUX", "x86_64", json!({})),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateFleet {display_name}: expected 200, got {status} with {body}"
    );
    string_at(&body, "fleetId")
}

/// The `fleetId`s one `ListFleets` page reports, in the order it reported them, plus the
/// token the next page starts at.
pub(crate) fn page_of(body: &Value) -> (Vec<String>, Option<String>) {
    let ids = body
        .get("fleets")
        .and_then(Value::as_array)
        .map(|fleets| {
            fleets
                .iter()
                .map(|fleet| string_at(fleet, "fleetId"))
                .collect()
        })
        .unwrap_or_default();
    let next = body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (ids, next)
}

#[tokio::test]
async fn create_fleet_then_get_fleet_returns_what_was_created() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("fleets").await;

    // The rule: GetFleet reports the values CreateFleet persisted under that farm. The
    // rows differ along the configuration union member, the operating system and
    // architecture, the worker-count window (both ends zero, a window, the maximum at its
    // published bound), the display-name length at both bounds, the description, tags, a
    // host-configuration script and the custom capabilities a configuration declares.
    let cases: Vec<(&str, Value)> = vec![
        (
            "customer managed, no workers wanted",
            json!({
                "displayName": "f",
                "roleArn": ROLE_ARN,
                "maxWorkerCount": 0,
                "configuration": customer_managed("LINUX", "x86_64", json!({})),
            }),
        ),
        (
            "customer managed, a window and a description",
            json!({
                "displayName": "fleet-b",
                "description": "the second fleet",
                "roleArn": ROLE_ARN,
                "minWorkerCount": 2,
                "maxWorkerCount": 9,
                "configuration": customer_managed("WINDOWS", "x86_64", json!({})),
                "tags": {"team": "lighting"},
            }),
        ),
        (
            "customer managed on macOS with custom capabilities",
            json!({
                "displayName": "c".repeat(100),
                "description": "",
                "roleArn": "arn:aws:iam::000000000000:role/path/to/DeadlineFleet",
                "minWorkerCount": 1,
                "maxWorkerCount": 1,
                "configuration": customer_managed(
                    "MACOS",
                    "arm64",
                    json!({
                        "customAmounts": [{"name": "amount.custom.pixels", "min": 1.0, "max": 4.0}],
                        "customAttributes": [
                            {"name": "attr.custom.renderer", "values": ["blender", "maya"]},
                        ],
                    }),
                ),
            }),
        ),
        (
            "service managed EC2 spot, scripted host",
            json!({
                "displayName": "fleet-d",
                "roleArn": ROLE_ARN,
                "minWorkerCount": 0,
                "maxWorkerCount": 2147483647,
                "configuration": service_managed("WINDOWS", "x86_64", "spot", json!({})),
                "hostConfiguration": {"scriptBody": "echo configuring", "scriptTimeoutSeconds": 600},
            }),
        ),
        (
            "service managed EC2 on-demand with custom amounts",
            json!({
                "displayName": "fleet-e",
                "roleArn": ROLE_ARN,
                "minWorkerCount": 3,
                "maxWorkerCount": 3,
                "configuration": service_managed(
                    "LINUX",
                    "arm64",
                    "on-demand",
                    json!({
                        "customAmounts": [{"name": "amount.custom.licenses", "min": 2.0}],
                    }),
                ),
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, created) = emulator
            .post(&create_fleet_path(&farm_id), request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateFleet to answer 200, got {status} with {created}"
        );
        let fleet_id = string_at(&created, "fleetId");
        assert!(
            is_resource_id(&fleet_id, "fleet"),
            "case {case:?}: expected a ^fleet-[0-9a-f]{{32}}$ id, got {fleet_id:?}"
        );

        let (status, fleet) = emulator.get(&get_fleet_path(&farm_id, &fleet_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetFleet to answer 200, got {status} with {fleet}"
        );
        assert_eq!(
            (
                string_at(&fleet, "fleetId"),
                string_at(&fleet, "farmId"),
                string_at(&fleet, "displayName"),
                string_at(&fleet, "roleArn"),
            ),
            (
                fleet_id.clone(),
                farm_id.clone(),
                string_at(&request, "displayName"),
                string_at(&request, "roleArn"),
            ),
            "case {case:?}: GetFleet reported another fleet's identity, body {fleet}"
        );
        // `description` defaults to "" and `minWorkerCount` to 0, per the model; an empty
        // description is nothing to report, so the member is left out.
        let expected_description = request
            .get("description")
            .and_then(Value::as_str)
            .filter(|description| !description.is_empty());
        assert_eq!(
            fleet.get("description").and_then(Value::as_str),
            expected_description,
            "case {case:?}: expected description {expected_description:?}, got {fleet}"
        );
        let min = request
            .get("minWorkerCount")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let max = request
            .get("maxWorkerCount")
            .and_then(Value::as_i64)
            .expect("maxWorkerCount is required");
        assert_eq!(
            (
                fleet.get("minWorkerCount").and_then(Value::as_i64),
                fleet.get("maxWorkerCount").and_then(Value::as_i64),
            ),
            (Some(min), Some(max)),
            "case {case:?}: expected the created worker-count window {min}..{max}, got {fleet}"
        );
        // Nothing has registered a worker, so the fleet holds none and is scaling
        // towards its minimum.
        assert_eq!(
            fleet.get("workerCount").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: expected no workers in a fleet nothing registered one with, got {fleet}"
        );
        assert_eq!(
            fleet.get("targetWorkerCount").and_then(Value::as_i64),
            Some(min.min(max)),
            "case {case:?}: expected a target of {}, got {fleet}",
            min.min(max)
        );
        let expected_auto_scaling = if min.min(max) == 0 { "STEADY" } else { "GROWING" };
        assert_eq!(
            string_at(&fleet, "autoScalingStatus"),
            expected_auto_scaling,
            "case {case:?}: expected autoScalingStatus {expected_auto_scaling}, got {fleet}"
        );
        assert_eq!(
            string_at(&fleet, "status"),
            "CREATE_IN_PROGRESS",
            "case {case:?}: expected a new fleet to still be creating, got {fleet}"
        );
        // `statusMessage` communicates a SUSPENDED fleet's reason, so nothing carries one
        // here.
        assert!(
            fleet.get("statusMessage").is_none(),
            "case {case:?}: expected no statusMessage on a fleet nothing suspended, got {fleet}"
        );

        // The configuration comes back as the union member the request used, with the
        // capability leaves it declared.
        let requested_configuration = request
            .get("configuration")
            .expect("configuration is required");
        let member = capabilities_path(requested_configuration);
        let reported_configuration = fleet
            .get("configuration")
            .expect("configuration is required in the response");
        for leaf in [
            "osFamily",
            "cpuArchitectureType",
            "vCpuCount.min",
            "memoryMiB.min",
            "customAmounts",
            "customAttributes",
        ] {
            let path = format!("{member}.{leaf}");
            assert_eq!(
                at(reported_configuration, &path),
                at(requested_configuration, &path),
                "case {case:?}: expected configuration {path} to round-trip, got {fleet}"
            );
        }

        // `capabilities` reports the capabilities the configuration NAMES, and a
        // configuration that names none reports none.
        let expected_amounts = at(requested_configuration, &format!("{member}.customAmounts"));
        let expected_attributes =
            at(requested_configuration, &format!("{member}.customAttributes"));
        assert_eq!(
            fleet
                .get("capabilities")
                .and_then(|capabilities| capabilities.get("amounts")),
            expected_amounts,
            "case {case:?}: expected the declared custom amounts as capabilities, got {fleet}"
        );
        assert_eq!(
            fleet
                .get("capabilities")
                .and_then(|capabilities| capabilities.get("attributes")),
            expected_attributes,
            "case {case:?}: expected the declared custom attributes as capabilities, got {fleet}"
        );

        assert_eq!(
            fleet.get("hostConfiguration"),
            request.get("hostConfiguration"),
            "case {case:?}: expected the created hostConfiguration, got {fleet}"
        );
        assert!(
            !string_at(&fleet, "createdBy").is_empty(),
            "case {case:?}: expected a createdBy principal, got {fleet}"
        );
        assert!(
            // Modelled `@timestampFormat("date-time")`: an RFC 3339 string, never the
            // protocol-default epoch number (`date_time_wire_form` owns that rule).
            fleet.get("createdAt").and_then(Value::as_str).is_some(),
            "case {case:?}: expected createdAt as an RFC 3339 string, got {fleet}"
        );
        assert!(
            fleet.get("updatedAt").is_none() && fleet.get("updatedBy").is_none(),
            "case {case:?}: expected no updatedAt/updatedBy before an update, got {fleet}"
        );

        // A read does not change what the next reader sees.
        let (_, again) = emulator.get(&get_fleet_path(&farm_id, &fleet_id)).await;
        assert_eq!(
            again, fleet,
            "case {case:?}: two reads of one fleet disagreed: {fleet} then {again}"
        );
    }
}

#[tokio::test]
async fn create_fleet_refuses_a_minimum_above_the_maximum() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("windows").await;

    // The rule: the fleet's minimum worker count is the floor it is scaled to and its
    // maximum the ceiling it never passes, so a minimum above the maximum is refused with
    // ValidationException; a minimum at or below the maximum is accepted. The rows walk
    // the relation across both published bounds of `MinZeroMaxInteger` (`range
    // 0..2147483647`) and both sides of equality.
    let cases: Vec<(&str, Option<i64>, i64, bool)> = vec![
        ("both bounds zero", Some(0), 0, true),
        ("the default minimum", None, 1, true),
        ("a minimum below the maximum", Some(1), 2, true),
        ("a minimum equal to the maximum", Some(7), 7, true),
        ("the maximum at its published bound", Some(2147483647), 2147483647, true),
        ("one above a zero maximum", Some(1), 0, false),
        ("a minimum two above the maximum", Some(4), 2, false),
        ("the published bound above a small maximum", Some(2147483647), 5, false),
    ];

    for (case, min, max, accepted) in cases {
        let mut request = json!({
            "displayName": "window",
            "roleArn": ROLE_ARN,
            "maxWorkerCount": max,
            "configuration": customer_managed("LINUX", "x86_64", json!({})),
        });
        if let Some(min) = min {
            request["minWorkerCount"] = json!(min);
        }
        let (status, body) = emulator.post(&create_fleet_path(&farm_id), request).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateFleet to answer 200, got {status} with {body}"
            );
            let fleet_id = string_at(&body, "fleetId");
            let (status, fleet) = emulator.get(&get_fleet_path(&farm_id, &fleet_id)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the accepted fleet to be readable, got {status} with \
                 {fleet}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateFleet to answer 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            string_at(&body, "reason"),
            "FIELD_VALIDATION_FAILED",
            "case {case:?}: expected the field-validation reason, got {body}"
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
        assert!(
            string_at(&body, "message").contains("maxWorkerCount"),
            "case {case:?}: expected the message to name the member it is compared against, \
             got {body}"
        );
        // The message names the window the request asked for, so its variable parts come
        // from the request rather than being fixed in the handler.
        let expected_message = format!(
            "minWorkerCount ({}) must be less than or equal to maxWorkerCount ({max})",
            min.unwrap_or(0)
        );
        assert_eq!(
            string_at(&body, "message"),
            expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn create_fleet_refuses_an_inverted_capability_range() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("ranges").await;

    // Where the request nests a fleet's capabilities, per the configuration union member:
    // a refusal names the pair's own path in the request.
    const CUSTOMER_CAPS: &str = "configuration.customerManaged.workerCapabilities";
    const SERVICE_CAPS: &str = "configuration.serviceManagedEc2.instanceCapabilities";

    // The same rule applied to the other lower/upper pairs a CreateFleet request carries:
    // the capability ranges its configuration declares. The rows walk each range the
    // model defines, on both union members, with the pair inverted, equal, open-ended and
    // in order.
    let cases: Vec<(&str, String, Value, bool)> = vec![
        (
            "a customer managed vCPU range in order",
            format!("{CUSTOMER_CAPS}.vCpuCount"),
            customer_managed("LINUX", "x86_64", json!({"vCpuCount": {"min": 1, "max": 8}})),
            true,
        ),
        (
            "a customer managed vCPU range with no upper bound",
            format!("{CUSTOMER_CAPS}.vCpuCount"),
            customer_managed("LINUX", "x86_64", json!({"vCpuCount": {"min": 8}})),
            true,
        ),
        (
            "an equal customer managed vCPU range",
            format!("{CUSTOMER_CAPS}.vCpuCount"),
            customer_managed("LINUX", "x86_64", json!({"vCpuCount": {"min": 4, "max": 4}})),
            true,
        ),
        (
            "an inverted customer managed vCPU range",
            format!("{CUSTOMER_CAPS}.vCpuCount"),
            customer_managed("LINUX", "x86_64", json!({"vCpuCount": {"min": 8, "max": 2}})),
            false,
        ),
        (
            "an inverted customer managed memory range",
            format!("{CUSTOMER_CAPS}.memoryMiB"),
            customer_managed(
                "LINUX",
                "x86_64",
                json!({"memoryMiB": {"min": 4096, "max": 1024}}),
            ),
            false,
        ),
        (
            "an inverted accelerator count",
            format!("{CUSTOMER_CAPS}.acceleratorCount"),
            customer_managed(
                "LINUX",
                "x86_64",
                json!({
                    "acceleratorTypes": ["gpu"],
                    "acceleratorCount": {"min": 4, "max": 1},
                }),
            ),
            false,
        ),
        (
            "an inverted accelerator memory total",
            format!("{CUSTOMER_CAPS}.acceleratorTotalMemoryMiB"),
            customer_managed(
                "LINUX",
                "x86_64",
                json!({"acceleratorTotalMemoryMiB": {"min": 8192, "max": 1024}}),
            ),
            false,
        ),
        (
            "an inverted service managed memory range",
            format!("{SERVICE_CAPS}.memoryMiB"),
            service_managed(
                "LINUX",
                "x86_64",
                "on-demand",
                json!({"memoryMiB": {"min": 8192, "max": 2048}}),
            ),
            false,
        ),
        (
            "an inverted service managed accelerator count",
            format!("{SERVICE_CAPS}.acceleratorCount"),
            service_managed(
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
            false,
        ),
        (
            "a custom amount in order",
            format!("{CUSTOMER_CAPS}.customAmounts[0]"),
            customer_managed(
                "LINUX",
                "x86_64",
                json!({"customAmounts": [{"name": "amount.custom.pixels", "min": 1.0, "max": 4.5}]}),
            ),
            true,
        ),
        (
            "a custom amount with no upper bound",
            format!("{CUSTOMER_CAPS}.customAmounts[0]"),
            customer_managed(
                "LINUX",
                "x86_64",
                json!({"customAmounts": [{"name": "amount.custom.pixels", "min": 4.5}]}),
            ),
            true,
        ),
        (
            "an inverted custom amount, named by its entry in the request",
            format!("{CUSTOMER_CAPS}.customAmounts[0]"),
            customer_managed(
                "LINUX",
                "x86_64",
                json!({"customAmounts": [{"name": "amount.custom.pixels", "min": 4.5, "max": 1.5}]}),
            ),
            false,
        ),
        (
            "an inverted custom amount on a service managed fleet",
            format!("{SERVICE_CAPS}.customAmounts[0]"),
            service_managed(
                "LINUX",
                "x86_64",
                "on-demand",
                json!({"customAmounts": [{"name": "amount.custom.licenses", "min": 8.0, "max": 2.0}]}),
            ),
            false,
        ),
    ];

    for (case, member, configuration, accepted) in cases {
        let (status, body) = emulator
            .post(
                &create_fleet_path(&farm_id),
                json!({
                    "displayName": "ranged",
                    "roleArn": ROLE_ARN,
                    "maxWorkerCount": 2,
                    "configuration": configuration,
                }),
            )
            .await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateFleet to answer 200, got {status} with {body}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateFleet to answer 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
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
        assert!(
            string_at(&body, "message").starts_with(&format!("{member}.min (")),
            "case {case:?}: expected the message to name the inverted pair, got {body}"
        );
    }
}

#[tokio::test]
async fn a_new_fleet_reaches_active_when_the_clock_makes_it_due() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("lifecycle").await;
    let (status, created) = emulator
        .post(
            &create_fleet_path(&farm_id),
            json!({
                "displayName": "lifecycle",
                "roleArn": ROLE_ARN,
                "minWorkerCount": 1,
                "maxWorkerCount": 4,
                "configuration": customer_managed("LINUX", "x86_64", json!({})),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateFleet: expected 200, got {status} with {created}"
    );
    let fleet_id = string_at(&created, "fleetId");
    let path = get_fleet_path(&farm_id, &fleet_id);

    // A caller reading the fleet straight back still sees the non-terminal status the
    // create left it in.
    let (status, fleet) = emulator.get(&path).await;
    assert_eq!(status, 200, "GetFleet: expected 200, got {status} with {fleet}");
    assert_eq!(
        string_at(&fleet, "status"),
        "CREATE_IN_PROGRESS",
        "immediately after CreateFleet: expected CREATE_IN_PROGRESS, got {fleet}"
    );

    // The transition is owed by the clock, not by any call: wait past the span and the
    // next read reports it.
    tokio::time::sleep(Duration::from_millis(3_400)).await;
    let (status, fleet) = emulator.get(&path).await;
    assert_eq!(status, 200, "GetFleet: expected 200, got {status} with {fleet}");
    assert_eq!(
        string_at(&fleet, "status"),
        "ACTIVE",
        "after the create span: expected ACTIVE, got {fleet}"
    );
    // The fleet still wants its minimum and still holds no worker, so it is still
    // growing.
    assert_eq!(
        (
            fleet.get("workerCount").and_then(Value::as_i64),
            fleet.get("targetWorkerCount").and_then(Value::as_i64),
            string_at(&fleet, "autoScalingStatus"),
        ),
        (Some(0), Some(1), "GROWING".to_string()),
        "after the create span: expected a fleet still growing towards its minimum, got {fleet}"
    );
}

#[tokio::test]
async fn one_client_token_creates_one_fleet() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("idempotent").await;
    let other_farm_id = emulator.farm("idempotent too").await;
    let request = json!({
        "displayName": "retried",
        "roleArn": ROLE_ARN,
        "maxWorkerCount": 5,
        "configuration": customer_managed("LINUX", "x86_64", json!({})),
    });

    // The rule: `clientToken` is `@idempotencyToken`, so a retry of the same create
    // answers with the resource the first call made. The rows differ along what changes
    // between the two calls: nothing, the token, or the farm the create is made under.
    let first = emulator
        .call(
            "POST",
            &create_fleet_path(&farm_id),
            Some(request.clone()),
            Some("token-one"),
        )
        .await;
    assert_eq!(first.0, 200, "first create: expected 200, got {:?}", first);
    let fleet_id = string_at(&first.1, "fleetId");

    let cases: Vec<(&str, String, &str, bool)> = vec![
        ("the same token on the same farm", farm_id.clone(), "token-one", true),
        ("another token on the same farm", farm_id.clone(), "token-two", false),
        (
            "the same token on another farm",
            other_farm_id.clone(),
            "token-one",
            false,
        ),
    ];

    for (case, farm, token, same) in cases {
        let (status, body) = emulator
            .call(
                "POST",
                &create_fleet_path(&farm),
                Some(request.clone()),
                Some(token),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateFleet to answer 200, got {status} with {body}"
        );
        let retried = string_at(&body, "fleetId");
        assert_eq!(
            retried == fleet_id,
            same,
            "case {case:?}: expected {} the first fleet {fleet_id}, got {retried}",
            if same { "exactly" } else { "another fleet than" }
        );
    }
}

#[tokio::test]
async fn a_fleet_is_reachable_only_through_its_own_farm() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("owner").await;
    let other_farm_id = emulator.farm("stranger").await;
    let (_, created) = emulator
        .post(
            &create_fleet_path(&farm_id),
            json!({
                "displayName": "owned",
                "roleArn": ROLE_ARN,
                "maxWorkerCount": 1,
                "configuration": customer_managed("LINUX", "x86_64", json!({})),
            }),
        )
        .await;
    let fleet_id = string_at(&created, "fleetId");
    let absent_farm = absent_id("farm", "00000000000000000000000000000001");
    let absent_fleet = absent_id("fleet", "00000000000000000000000000000002");

    // The rule: a fleet hangs off its farm, so a read naming another farm, an unheld
    // farm or an unheld fleet is a 404 naming the level that is missing.
    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "a fleet of another farm",
            other_farm_id.clone(),
            fleet_id.clone(),
            "fleet",
            fleet_id.clone(),
        ),
        (
            "an unheld farm",
            absent_farm.clone(),
            fleet_id.clone(),
            "farm",
            absent_farm.clone(),
        ),
        (
            "an unheld fleet",
            farm_id.clone(),
            absent_fleet.clone(),
            "fleet",
            absent_fleet.clone(),
        ),
    ];

    for (case, farm, fleet, resource_type, resource_id) in cases {
        let (status, body) = emulator.get(&get_fleet_path(&farm, &fleet)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetFleet to answer 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            (
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId"),
            ),
            (resource_type.to_string(), resource_id.clone()),
            "case {case:?}: expected the 404 to name what is missing, got {body}"
        );
    }

    // A create under an unheld farm is the same 404, and creates nothing.
    let (status, body) = emulator
        .post(
            &create_fleet_path(&absent_farm),
            json!({
                "displayName": "orphan",
                "roleArn": ROLE_ARN,
                "maxWorkerCount": 1,
                "configuration": customer_managed("LINUX", "x86_64", json!({})),
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "CreateFleet under an unheld farm: expected 404, got {status} with {body}"
    );
    assert_eq!(
        (
            error_code(&body),
            string_at(&body, "resourceType"),
            string_at(&body, "resourceId"),
        ),
        (
            "ResourceNotFoundException".to_string(),
            "farm".to_string(),
            absent_farm.clone(),
        ),
        "CreateFleet under an unheld farm: expected the 404 to name the farm, got {body}"
    );
}

#[tokio::test]
async fn list_fleets_pages_and_filters_the_fleets_of_one_farm() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("listed").await;
    let other_farm_id = emulator.farm("unlisted").await;

    // Two fleets share a display name, so the display-name filter is a filter and not an
    // id lookup.
    let names = ["alpha", "beta", "gamma", "delta", "alpha"];
    let mut created = Vec::new();
    for name in names {
        created.push(create_fleet(&emulator, &farm_id, name).await);
    }
    let stranger = create_fleet(&emulator, &other_farm_id, "alpha").await;

    // The rule: the list reports the fleets of the farm in its path, oldest first.
    let (status, body) = emulator.get(&create_fleet_path(&farm_id)).await;
    assert_eq!(
        status, 200,
        "ListFleets: expected 200, got {status} with {body}"
    );
    let (listed, next) = page_of(&body);
    assert_eq!(
        (listed.clone(), next),
        (created.clone(), None),
        "unpaged: expected every fleet of this farm oldest first, got {body}"
    );
    assert!(
        !listed.contains(&stranger),
        "unpaged: another farm's fleet {stranger} was listed, got {body}"
    );
    let summary = body
        .get("fleets")
        .and_then(Value::as_array)
        .and_then(|fleets| fleets.first())
        .cloned()
        .expect("a summary per fleet");
    assert_eq!(
        (
            string_at(&summary, "farmId"),
            string_at(&summary, "displayName"),
            string_at(&summary, "status"),
            summary.get("workerCount").and_then(Value::as_i64),
            summary.get("maxWorkerCount").and_then(Value::as_i64),
        ),
        (
            farm_id.clone(),
            "alpha".to_string(),
            "CREATE_IN_PROGRESS".to_string(),
            Some(0),
            Some(4),
        ),
        "unpaged: expected the summary to carry what the create persisted, got {summary}"
    );
    assert!(
        summary.get("configuration").is_some() && !string_at(&summary, "createdBy").is_empty(),
        "unpaged: expected a configuration and a createdBy in the summary, got {summary}"
    );

    // Paged: the rows differ along the page size, from one fleet at a time up to more
    // than this farm holds.
    for page_size in [1, 2, 3, 5, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("?maxResults={page_size}"),
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
            };
            let (status, body) = emulator
                .get(&format!("{}{query}", create_fleet_path(&farm_id)))
                .await;
            assert_eq!(
                status, 200,
                "page size {page_size}: expected 200, got {status} with {body}"
            );
            let (ids, next) = page_of(&body);
            assert!(
                ids.len() <= page_size.min(created.len()),
                "page size {page_size}: expected at most {page_size} fleets, got {body}"
            );
            walked.extend(ids);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "page size {page_size}: paging did not terminate, walked {walked:?}"
            );
            match next {
                None => break,
                Some(next) => token = Some(next),
            }
        }
        assert_eq!(
            walked, created,
            "page size {page_size}: expected paging to walk every fleet once, in order"
        );
    }

    // A token no page handed out is a client error, not an empty page.
    let (status, body) = emulator
        .get(&format!(
            "{}?nextToken={}",
            create_fleet_path(&farm_id),
            absent_id("fleet", "0000000000000000000000000000000f")
        ))
        .await;
    assert_eq!(
        status, 400,
        "an unknown nextToken: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "an unknown nextToken: expected a ValidationException, got {body}"
    );

    // Filters: each one selects by the member it names, and they combine.
    let expected_alphas: Vec<String> = names
        .iter()
        .zip(created.iter())
        .filter(|(name, _)| **name == "alpha")
        .map(|(_, id)| id.clone())
        .collect();
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "one display name shared by two fleets",
            "?displayName=alpha".to_string(),
            expected_alphas.clone(),
        ),
        (
            "a display name one fleet carries",
            "?displayName=beta".to_string(),
            vec![created[1].clone()],
        ),
        (
            "a display name nothing carries",
            "?displayName=epsilon".to_string(),
            Vec::new(),
        ),
        (
            "the status every new fleet carries",
            "?status=CREATE_IN_PROGRESS".to_string(),
            created.clone(),
        ),
        (
            "a status nothing has reached yet",
            "?status=ACTIVE".to_string(),
            Vec::new(),
        ),
        (
            "a display name and a matching status",
            "?displayName=alpha&status=CREATE_IN_PROGRESS".to_string(),
            expected_alphas.clone(),
        ),
        (
            "a display name and a status nothing combines",
            "?displayName=alpha&status=SUSPENDED".to_string(),
            Vec::new(),
        ),
    ];
    for (case, query, expected) in cases {
        let (status, body) = emulator
            .get(&format!("{}{query}", create_fleet_path(&farm_id)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListFleets to answer 200, got {status} with {body}"
        );
        let (ids, _) = page_of(&body);
        assert_eq!(
            ids, expected,
            "case {case:?}: expected exactly the matching fleets, got {body}"
        );
    }

    // The status the clock made due is what the list reports: once the create span has
    // passed, the same status filters select the other way round.
    tokio::time::sleep(Duration::from_millis(3_400)).await;
    for (case, query, expected) in [
        ("active after the span", "?status=ACTIVE", created.clone()),
        (
            "no longer creating",
            "?status=CREATE_IN_PROGRESS",
            Vec::new(),
        ),
    ] {
        let (status, body) = emulator
            .get(&format!("{}{query}", create_fleet_path(&farm_id)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListFleets to answer 200, got {status} with {body}"
        );
        let (ids, _) = page_of(&body);
        assert_eq!(
            ids, expected,
            "case {case:?}: expected the clock-owed status to be reported, got {body}"
        );
    }

    // A list under a farm this emulator does not hold is a 404, not an empty page.
    let absent_farm = absent_id("farm", "0000000000000000000000000000000e");
    let (status, body) = emulator.get(&create_fleet_path(&absent_farm)).await;
    assert_eq!(
        status, 404,
        "ListFleets under an unheld farm: expected 404, got {status} with {body}"
    );
    assert_eq!(
        (
            error_code(&body),
            string_at(&body, "resourceType"),
            string_at(&body, "resourceId"),
        ),
        (
            "ResourceNotFoundException".to_string(),
            "farm".to_string(),
            absent_farm.clone(),
        ),
        "ListFleets under an unheld farm: expected the 404 to name the farm, got {body}"
    );

    // The principalId filter asks for the fleets a principal is a member of, which this
    // emulator does not persist yet: that is answered 501, never with an unfiltered list.
    let (status, body) = emulator
        .get(&format!(
            "{}?principalId=0123456789-abcdefab-1234-5678-9abc-abcdefabcdef",
            create_fleet_path(&farm_id)
        ))
        .await;
    assert_eq!(
        status, 501,
        "a principalId filter: expected 501, got {status} with {body}"
    );
}
