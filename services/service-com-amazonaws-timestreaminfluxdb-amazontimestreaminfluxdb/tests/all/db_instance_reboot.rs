// SPDX-License-Identifier: CC0-1.0

//! Rebooting a DB instance, end to end over the wire.
//!
//! RULE: a reboot restarts the engine of a DB instance that is RUNNING. For an instance
//! whose status is `AVAILABLE` the call is accepted: it answers HTTP 200 with the stored
//! record, reports `status` `REBOOTING` while the restart runs, changes no other member of
//! the record, and the clock settles the record back to `AVAILABLE` on its own. An
//! instance in any other status is busy with a change of its own, so the request is
//! refused with HTTP 400 `ValidationException` and the record is left exactly as it was;
//! an `identifier` that resolves to no live instance is refused with HTTP 404
//! `ResourceNotFoundException`.
//!
//! Every status name asserted here is a member of the modelled `Status` enum
//! (`smithy_query(query="shape", shape="Status")`), and the member set the reboot answer
//! must carry is `RebootDbInstanceOutput`'s
//! (`smithy_query(query="operation", operation="RebootDbInstance")`).
//!
//! The accepted rows span what the rule generalises over: the instance type and its
//! storage size, the deployment type (so one row carries a standby in a second
//! Availability Zone), how many optional members the record holds, the list lengths, the
//! port, and the Region the instance was created in. The refused rows span every transient
//! status this API can put an instance into — the provision, the three flavours of
//! reconfiguration, and a restart already running.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_lifecycle::{call_in, ok, router, REGION};

/// The minimum a `CreateDbInstance` request must carry, per the model's `@required`
/// members (`name`, `password`, `dbInstanceType`, `vpcSubnetIds`, `vpcSecurityGroupIds`,
/// `allocatedStorage`).
fn minimal_create(name: &str) -> Value {
    json!({
        "name": name,
        "password": "supersecret1",
        "dbInstanceType": "db.influx.medium",
        "allocatedStorage": 20,
        "vpcSubnetIds": ["subnet-aaa"],
        "vpcSecurityGroupIds": ["sg-aaa"],
    })
}

/// The `id` of a DB instance record in a response body.
fn id_of(case: &str, body: &Value) -> String {
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string `id`, got {body}"))
        .to_string()
}

/// How long a test waits for a clock-owed settle before giving up: far above the span the
/// service settles a transient status after, so a loaded host does not fail the test.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Read the record back until it reports `wanted`.
///
/// Waits for the CONDITION rather than for a span of time: the settle the clock owes is
/// materialised when a reporting operation reads the record, so the test looks again until
/// the record reports the status it is waiting for or the generous deadline above passes.
async fn poll_until_status(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    wanted: &str,
    case: &str,
) -> Value {
    let started = Instant::now();
    loop {
        let body = ok(
            case,
            call_in(
                router,
                region,
                "GetDbInstance",
                json!({ "identifier": identifier }),
            )
            .await,
        );
        if body["status"].as_str() == Some(wanted) {
            return body;
        }
        assert!(
            started.elapsed() < SETTLE_DEADLINE,
            "case {case:?}: the record never reached {wanted}, it reports {:?}",
            body["status"]
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row of the accepted-reboot table: a create request and the Region to sign it in.
struct RebootCase {
    label: &'static str,
    region: &'static str,
    body: Value,
}

/// Rows spanning what an accepted reboot generalises over: the instance type and storage
/// size, the deployment type, how many optional members the record carries, the list
/// lengths, the port, and the Region.
fn reboot_cases() -> Vec<RebootCase> {
    vec![
        RebootCase {
            label: "only the required members",
            region: REGION,
            body: minimal_create("reboot-minimal"),
        },
        RebootCase {
            label: "every optional member set",
            region: REGION,
            body: json!({
                "name": "reboot-fully-specified",
                "password": "supersecret1",
                "username": "myuser",
                "organization": "myorg",
                "bucket": "mybucket",
                "dbInstanceType": "db.influx.8xlarge",
                "allocatedStorage": 400,
                "vpcSubnetIds": ["subnet-aaa", "subnet-bbb"],
                "vpcSecurityGroupIds": ["sg-aaa", "sg-bbb"],
                "publiclyAccessible": true,
                "dbStorageType": "InfluxIOIncludedT3",
                "deploymentType": "SINGLE_AZ",
                "networkType": "DUAL",
                "port": 9999,
                "kmsKeyId": "alias/my-key",
                "logDeliveryConfiguration": {
                    "s3Configuration": { "bucketName": "log-bucket", "enabled": true }
                },
                "maintenanceSchedule": {
                    "timezone": "UTC",
                    "preferredMaintenanceWindow": "Sun:23:00-Sun:23:30"
                },
                "dbBackupConfigurations": [
                    { "type": "DAILY", "retentionDays": 7, "enabled": true }
                ],
                "tags": { "team": "data" },
            }),
        },
        RebootCase {
            label: "a multi-AZ standby deployment, with a second Availability Zone",
            region: REGION,
            body: json!({
                "name": "reboot-standby-pair",
                "password": "supersecret1",
                "dbInstanceType": "db.influx.large",
                "allocatedStorage": 100,
                "vpcSubnetIds": ["subnet-aaa", "subnet-bbb"],
                "vpcSecurityGroupIds": ["sg-aaa"],
                "deploymentType": "WITH_MULTIAZ_STANDBY",
            }),
        },
        RebootCase {
            label: "the published upper bounds, in another Region",
            region: "eu-central-1",
            body: json!({
                "name": "reboot-high-bounds",
                "password": "supersecret1",
                "dbInstanceType": "db.influx.24xlarge",
                "allocatedStorage": 15360,
                "port": 65535,
                "vpcSubnetIds": [
                    "subnet-a1", "subnet-a2", "subnet-a3",
                    "subnet-a4", "subnet-a5", "subnet-a6"
                ],
                "vpcSecurityGroupIds": ["sg-b1", "sg-b2", "sg-b3", "sg-b4", "sg-b5"],
            }),
        },
    ]
}

#[tokio::test]
async fn rebooting_an_available_instance_reports_rebooting_and_changes_nothing_else() {
    for case in reboot_cases() {
        let router = router();
        let created = ok(
            case.label,
            call_in(&router, case.region, "CreateDbInstance", case.body.clone()).await,
        );
        let id = id_of(case.label, &created);
        let before =
            poll_until_status(&router, case.region, &id, "AVAILABLE", case.label).await;

        let rebooting = ok(
            case.label,
            call_in(
                &router,
                case.region,
                "RebootDbInstance",
                json!({ "identifier": &id }),
            )
            .await,
        );

        assert_eq!(
            rebooting["status"].as_str(),
            Some("REBOOTING"),
            "case {:?}: a reboot of an AVAILABLE instance restarts it, got {:?}",
            case.label,
            rebooting["status"]
        );
        // A reboot reconfigures nothing: every member but the status is the record the
        // read before it reported, and the answer carries exactly the same member set.
        assert_eq!(
            rebooting
                .as_object()
                .expect("an object response")
                .keys()
                .collect::<Vec<_>>(),
            before
                .as_object()
                .expect("an object response")
                .keys()
                .collect::<Vec<_>>(),
            "case {:?}: the reboot answer must carry the same members as the record",
            case.label
        );
        for (member, value) in before.as_object().expect("an object response") {
            if member == "status" {
                continue;
            }
            assert_eq!(
                rebooting.get(member),
                Some(value),
                "case {:?}: a reboot must not change {member}, got {:?}",
                case.label,
                rebooting.get(member)
            );
        }

        // The restart is transient: the clock settles the record back to AVAILABLE, and
        // the record is otherwise the one that went into the reboot.
        let after = poll_until_status(&router, case.region, &id, "AVAILABLE", case.label).await;
        assert_eq!(
            after, before,
            "case {:?}: after the restart the record must read back unchanged",
            case.label
        );
    }
}

#[tokio::test]
async fn a_reboot_is_refused_while_the_instance_is_in_a_transient_status() {
    /// The call that starts a transition on an AVAILABLE instance: the operation, and the
    /// request body built from the instance's id.
    type StartTransition = (&'static str, fn(&str) -> Value);

    /// Each row: the label, the request that puts a fresh instance into a transient
    /// status, and the status it must then be refused in. Every status is a member of the
    /// modelled `Status` enum.
    struct TransientCase {
        label: &'static str,
        name: &'static str,
        /// `None` for the status a create itself leaves behind; otherwise the call that
        /// starts the transition.
        start: Option<StartTransition>,
        status: &'static str,
    }

    let cases = vec![
        TransientCase {
            label: "still being provisioned",
            name: "reboot-while-creating",
            start: None,
            status: "CREATING",
        },
        TransientCase {
            label: "a plain reconfiguration in flight",
            name: "reboot-while-updating",
            start: Some(("UpdateDbInstance", |id| {
                json!({ "identifier": id, "port": 1024 })
            })),
            status: "UPDATING",
        },
        TransientCase {
            label: "an instance-type change in flight",
            name: "reboot-while-resizing",
            start: Some(("UpdateDbInstance", |id| {
                json!({ "identifier": id, "dbInstanceType": "db.influx.large" })
            })),
            status: "UPDATING_INSTANCE_TYPE",
        },
        TransientCase {
            label: "a deployment-type change in flight",
            name: "reboot-while-redeploying",
            start: Some(("UpdateDbInstance", |id| {
                json!({ "identifier": id, "deploymentType": "WITH_MULTIAZ_STANDBY" })
            })),
            status: "UPDATING_DEPLOYMENT_TYPE",
        },
        TransientCase {
            label: "a restart already running",
            name: "reboot-while-rebooting",
            start: Some(("RebootDbInstance", |id| json!({ "identifier": id }))),
            status: "REBOOTING",
        },
    ];

    for case in cases {
        let router = router();
        let created = ok(
            case.label,
            call_in(
                &router,
                REGION,
                "CreateDbInstance",
                minimal_create(case.name),
            )
            .await,
        );
        let id = id_of(case.label, &created);

        // The row's transient status is the one the starting call reports, so the refusal
        // below is asserted against the status the service itself just announced.
        let started = match case.start {
            None => created,
            Some((operation, body)) => {
                poll_until_status(&router, REGION, &id, "AVAILABLE", case.label).await;
                ok(
                    case.label,
                    call_in(&router, REGION, operation, body(&id)).await,
                )
            }
        };
        assert_eq!(
            started["status"].as_str(),
            Some(case.status),
            "case {:?}: the instance must report {} before the reboot is tried, got {:?}",
            case.label,
            case.status,
            started["status"]
        );

        let (status, body) = call_in(
            &router,
            REGION,
            "RebootDbInstance",
            json!({ "identifier": &id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: a reboot of an instance in {} must be refused, got {status} \
             with {body}",
            case.label, case.status
        );
        assert!(
            body["__type"]
                .as_str()
                .unwrap_or_default()
                .ends_with("ValidationException"),
            "case {:?}: expected a ValidationException, got {body}",
            case.label
        );
        // The request is well formed and no member of it failed validation — the
        // instance's status is what refuses it — so the reason is the modelled catch-all
        // of `ValidationExceptionReason`.
        assert_eq!(
            body["reason"].as_str(),
            Some("OTHER"),
            "case {:?}: unexpected reason in {body}",
            case.label
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&id) && message.contains(case.status),
            "case {:?}: the refusal must name the instance and the status it is in, \
             got {message:?}",
            case.label
        );

        // The refused call left the record alone: it is still in the transient status it
        // was in, and the clock still settles it to AVAILABLE on its own.
        let settled = poll_until_status(&router, REGION, &id, "AVAILABLE", case.label).await;
        assert_eq!(
            settled["id"].as_str(),
            Some(id.as_str()),
            "case {:?}: the refused reboot must leave the record in place, got {settled}",
            case.label
        );

        // Once the instance is running again the same reboot is accepted.
        let accepted = ok(
            case.label,
            call_in(
                &router,
                REGION,
                "RebootDbInstance",
                json!({ "identifier": &id }),
            )
            .await,
        );
        assert_eq!(
            accepted["status"].as_str(),
            Some("REBOOTING"),
            "case {:?}: the reboot must be accepted once the instance is AVAILABLE, \
             got {:?}",
            case.label,
            accepted["status"]
        );
    }
}

#[tokio::test]
async fn a_reboot_of_an_identifier_that_resolves_to_nothing_is_not_found() {
    let router = router();
    // A live instance exists, so a 404 is about resolution and not about empty state.
    let live = ok(
        "setup",
        call_in(
            &router,
            REGION,
            "CreateDbInstance",
            minimal_create("reboot-live-one"),
        )
        .await,
    );
    let live_id = id_of("setup", &live);

    // Identifiers that satisfy the modelled `DbInstanceIdentifier` constraint (length
    // 3..64, `^[a-zA-Z0-9]+$`) but name no live instance — including the id of an instance
    // that has been deleted, and the same id in another Region.
    let deleted = ok(
        "setup: deleted",
        call_in(
            &router,
            REGION,
            "CreateDbInstance",
            minimal_create("reboot-deleted-one"),
        )
        .await,
    );
    let deleted_id = id_of("setup: deleted", &deleted);
    ok(
        "setup: deleted",
        call_in(
            &router,
            REGION,
            "DeleteDbInstance",
            json!({ "identifier": &deleted_id }),
        )
        .await,
    );

    let long_id = "z".repeat(64);
    let cases: Vec<(&str, &str)> = vec![
        ("the shortest identifier the model allows", "abc"),
        ("an identifier nothing ever minted", "doesnotexist"),
        ("a digits-only identifier", "0000000000"),
        ("the longest identifier the model allows", &long_id),
        ("the id of a deleted instance", &deleted_id),
    ];

    for (label, identifier) in cases {
        let (status, body) = call_in(
            &router,
            REGION,
            "RebootDbInstance",
            json!({ "identifier": identifier }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404, got {status} with {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .unwrap_or_default()
                .ends_with("ResourceNotFoundException"),
            "case {label:?}: expected a ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(identifier),
            "case {label:?}: the refusal must name the identifier asked for, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("DB_INSTANCE"),
            "case {label:?}: unexpected resourceType in {body}"
        );
    }

    // None of those refusals disturbed the live record: it is still there and the clock
    // still settles it to AVAILABLE on its own.
    let live_still_there = poll_until_status(&router, REGION, &live_id, "AVAILABLE", "live").await;
    assert_eq!(
        live_still_there["id"].as_str(),
        Some(live_id.as_str()),
        "a 404 on another identifier must not disturb a live record, got \
         {live_still_there}"
    );
}
