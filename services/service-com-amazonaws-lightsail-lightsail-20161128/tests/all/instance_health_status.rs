// SPDX-License-Identifier: CC0-1.0

//! The health of a Lightsail instance's status checks, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: an instance's two health members are reported only when
//! the reading request sets `includeHealthStatus`, and what they report follows
//! the instance's own lifecycle state — a serving instance's checks have passed,
//! one the clock is still bringing up has checks in progress, and one that is
//! not running has no check that applies.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{wait_for_state, Emulator};

/// The blueprint and bundle every instance in this suite is created on, ids of
/// the service's published catalogs (`GetBlueprints`, `GetBundles`).
const BLUEPRINT_ID: &str = "amazon_linux_2023";
const BUNDLE_ID: &str = "nano_3_0";

/// The zone every instance in this suite is created in.
const ZONE: &str = "us-east-1a";

/// `InstanceHealthStatus$status` values, from the modelled
/// `InstanceHealthStatusValue` enum and that member's documentation.
const OK: &str = "ok";
const INITIALIZING: &str = "initializing";
const NOT_APPLICABLE: &str = "not-applicable";

/// `InstanceHealthStatusDetail$name`'s only value, from the modelled
/// `InstanceHealthStatusDetailName` enum.
const REACHABILITY: &str = "reachability";

/// `InstanceHealthStatusDetail$status` values, from the modelled
/// `InstanceHealthStatusDetailValue` enum.
const PASSED: &str = "passed";
const DETAIL_INITIALIZING: &str = "initializing";

/// The two members of `Instance` that report an instance's health, so a case
/// asserts both rather than one.
const HEALTH_MEMBERS: [&str; 2] = ["instanceStatus", "systemStatus"];

/// Create the instance `name`, answering nothing: whether it is already running
/// is each case's own business.
async fn create_instance(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": ZONE,
                "blueprintId": BLUEPRINT_ID,
                "bundleId": BUNDLE_ID,
            }),
        )
        .await;
}

/// The instance `name` as `GetInstance` reports it, with `include` deciding
/// whether the request asks for the health of its checks.
async fn instance(emulator: &Emulator, name: &str, include: Option<bool>) -> Value {
    let mut body = json!({"instanceName": name});
    if let Some(include) = include {
        body["includeHealthStatus"] = json!(include);
    }
    emulator.ok("GetInstance", body).await["instance"].clone()
}

/// The instance `name` as `GetInstances` reports it, with the same flag.
async fn listed_instance(emulator: &Emulator, name: &str, include: Option<bool>) -> Value {
    let mut body = json!({});
    if let Some(include) = include {
        body["includeHealthStatus"] = json!(include);
    }
    let listing = emulator.ok("GetInstances", body).await;
    listing["instances"]
        .as_array()
        .expect("the listing reports an array")
        .iter()
        .find(|entry| entry["name"].as_str() == Some(name))
        .cloned()
        .unwrap_or(Value::Null)
}

/// RULE: the health members are reported only when the request sets
/// `includeHealthStatus` to true — on the read of one instance and on the
/// listing alike.
#[tokio::test]
async fn health_is_reported_only_when_the_request_asks_for_it() {
    let emulator = Emulator::new();
    create_instance(&emulator, "health-flag").await;
    wait_for_state(&emulator, "health-flag", "running").await;

    for asked in [None, Some(false), Some(true)] {
        let case = format!("includeHealthStatus {asked:?}");
        for (source, reported) in [
            ("GetInstance", instance(&emulator, "health-flag", asked).await),
            (
                "GetInstances",
                listed_instance(&emulator, "health-flag", asked).await,
            ),
        ] {
            for member in HEALTH_MEMBERS {
                let value = &reported[member];
                if asked == Some(true) {
                    assert!(
                        value["status"].as_str().is_some(),
                        "case {case}: {source} reports {member}, got {reported:?}"
                    );
                } else {
                    assert!(
                        value.is_null(),
                        "case {case}: {source} leaves {member} out, got {reported:?}"
                    );
                }
            }
        }
    }
}

/// RULE: the health an instance reports follows its own lifecycle state — a
/// serving instance's reachability check has passed, one still coming up has a
/// check in progress, and one that is not running has no check that applies and
/// so reports no check result.
#[tokio::test]
async fn health_follows_the_instance_lifecycle_state() {
    let emulator = Emulator::new();

    // Every state an instance of this store settles in or passes through, with
    // the health the rule says it then reports.
    //
    // `pending` is the transient state a create leaves behind, so the case
    // reads the instance straight after the create and retries when the clock
    // beat it; the settled states are waited for.
    for (case, state, status, detail) in [
        ("a serving instance", "running", OK, Some(PASSED)),
        (
            "a stopped instance",
            "stopped",
            NOT_APPLICABLE,
            None,
        ),
    ] {
        let name = format!("health-{state}");
        create_instance(&emulator, &name).await;
        wait_for_state(&emulator, &name, "running").await;
        if state == "stopped" {
            emulator
                .ok("StopInstance", json!({"instanceName": name}))
                .await;
            wait_for_state(&emulator, &name, "stopped").await;
        }
        let reported = instance(&emulator, &name, Some(true)).await;
        for member in HEALTH_MEMBERS {
            let health = &reported[member];
            assert_eq!(
                health["status"].as_str(),
                Some(status),
                "case {case}: {member} follows the instance state, got {reported:?}"
            );
            let details: Vec<(Option<&str>, Option<&str>)> = health["details"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case}: {member} must report a details array, got {reported:?}")
                })
                .iter()
                .map(|entry| (entry["name"].as_str(), entry["status"].as_str()))
                .collect();
            assert_eq!(
                details,
                detail
                    .map(|detail| vec![(Some(REACHABILITY), Some(detail))])
                    .unwrap_or_default(),
                "case {case}: {member} reports the check result the state gives it, got \
                 {reported:?}"
            );
            assert!(
                health["details"]
                    .as_array()
                    .is_some_and(|entries| entries.iter().all(|entry| entry["impairedSince"].is_null())),
                "case {case}: no check of this store has failed, got {reported:?}"
            );
        }
    }

    // The transient state a create leaves behind: the checks are in progress
    // while the instance is still coming up. The window is the one the clock
    // owes a new instance, so an attempt the clock beat is retried rather than
    // counted.
    const ATTEMPTS: usize = 10;
    let mut observed = false;
    for attempt in 0..ATTEMPTS {
        let name = format!("health-pending-{attempt}");
        create_instance(&emulator, &name).await;
        let reported = instance(&emulator, &name, Some(true)).await;
        if reported["state"]["name"].as_str() != Some("pending") {
            continue;
        }
        for member in HEALTH_MEMBERS {
            assert_eq!(
                (
                    reported[member]["status"].as_str(),
                    reported[member]["details"][0]["name"].as_str(),
                    reported[member]["details"][0]["status"].as_str(),
                ),
                (
                    Some(INITIALIZING),
                    Some(REACHABILITY),
                    Some(DETAIL_INITIALIZING)
                ),
                "case an instance still coming up: {member} reports a check in progress, got \
                 {reported:?}"
            );
        }
        observed = true;
        break;
    }
    assert!(
        observed,
        "case an instance still coming up: no attempt of {ATTEMPTS} observed the pending state"
    );

    // Nothing the health reads changed: the instance that was running still is,
    // which is what says a read reported the health rather than moving it.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reported = instance(&emulator, "health-running", Some(true)).await;
        if reported["state"]["name"].as_str() == Some("running") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case a read leaves the instance alone: expected it still running, got {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
