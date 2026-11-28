// SPDX-License-Identifier: CC0-1.0

//! What EVERY call that MOVES an application's own `Status` answers for an application
//! whose registration has SETTLED as failed.
//!
//! RULE under test: a registration that settled as failed discovered no SAP system, so a
//! call that moves that application's own `Status` has nothing to move and is refused with
//! the modelled `ValidationException` (HTTP 400) naming the application the request
//! carried — decided from the application's own registration outcome alone, so every call
//! of that class answers the same way, a second caller is answered exactly as the first,
//! and the refused call files no work and leaves the application reporting what it did.
//!
//! The class is every operation of this service that moves `Application$Status`:
//! `StartApplication` and `StopApplication` (`smithy_query(query="digest")` binds them to
//! `POST /start-application` and `POST /stop-application`, and both model the
//! `ValidationException` the refusal is). The calls whose precondition is the STRONGER one
//! — a registration that completed successfully, which `StartApplicationRefresh` and
//! `StartConfigurationChecks` state as `ConflictException` (409) — are a different rule,
//! pinned in `tests/all/registration_outcome.rs`.
//!
//! The rows span what the rule generalises over: which member of the class is called, the
//! switches that member admits beyond the application it names, the forms the modelled
//! `ApplicationId` admits, and how many times the refused call is repeated. The control of
//! every row is the SAME call against an application whose registration SUCCEEDED, which
//! has to be served — so the refusal is about the registration outcome and not about the
//! call or its switches.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns, with the owner of the instances mounted beside this service), and nothing here
//! waits a fixed time: each outcome the clock owes an application is waited for as a
//! CONDITION ([`wait_until`]).

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::application_lifecycle::rendered;
use crate::database_status_discovery::wait_until;
use crate::ec2_instance_shutdown::{registration, Emulator as WithEc2, UNHELD_INSTANCE};
use crate::registration_outcome::{failed_registration, records_of_type};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// One member of the class: a call that moves the application's own `Status`, with the
/// switches this row carries beyond the application it names.
#[derive(Debug)]
struct StatusMove {
    /// The URI the call is bound to.
    path: &'static str,
    /// What the request carries beyond its `ApplicationId`, as a JSON object.
    switches: &'static str,
    /// The `ApplicationId` of this row's control: the same call against an application
    /// whose registration SUCCEEDED.
    control_id: &'static str,
}

/// The rows: both members of the class, and the stop once more carrying every switch it
/// admits — the connected entity and the Amazon EC2 shutdown, which act on state outside
/// the application itself.
const STATUS_MOVES: &[StatusMove] = &[
    StatusMove {
        path: "/start-application",
        switches: "{}",
        control_id: "move-control-start",
    },
    StatusMove {
        path: "/stop-application",
        switches: "{}",
        control_id: "move-control-stop",
    },
    StatusMove {
        path: "/stop-application",
        switches: r#"{"StopConnectedEntity": "DBMS", "IncludeEc2InstanceShutdown": true}"#,
        control_id: "move-control-stop-switches",
    },
];

/// The applications whose registration settles as FAILED, spanning the forms the modelled
/// `ApplicationId` admits (`smithy_query(query="shape",shape="ApplicationId")`: length
/// 1..60, pattern `^[\w\d\.-]+$`) — the shortest the length admits, a mixed-case id
/// carrying both punctuation marks the pattern admits, and a plain one.
const FAILED_IDS: &[&str] = &["move-failed", "MOVE.FAILED-2", "m"];

/// How many times each refused call is repeated: a settled registration is no condition a
/// call consumes, so the answer does not change with the attempt.
const ATTEMPTS: u32 = 2;

/// The request body of `row` against `application_id`.
fn request(row: &StatusMove, application_id: &str) -> Value {
    let mut body: Map<String, Value> = serde_json::from_str(row.switches)
        .unwrap_or_else(|error| panic!("case {:?}: the switches parse: {error}", row.switches));
    body.insert("ApplicationId".into(), json!(application_id));
    Value::Object(body)
}

/// Register `application_id` on `instance`, which the registration's own discovery is then
/// run against.
async fn register(emulator: &WithEc2, application_id: &str, instance: &str) {
    let reply = emulator
        .ssm_sap(
            "/register-application",
            registration(application_id, instance, &[]),
        )
        .await;
    assert_eq!(
        reply.get("Application").and_then(|app| app.get("Status")),
        Some(&json!("REGISTERING")),
        "case {application_id:?}: an asynchronous registration answers REGISTERING, got {reply}"
    );
}

/// What `GetApplication` reports for `application_id`, as a value — the witness read the
/// refusals are judged against, so it is required to have been answered.
async fn reported(emulator: &WithEc2, application_id: &str) -> String {
    let reply = emulator
        .reply("/get-application", json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {application_id:?}: the witness read must be answered, got {reply:?}"
    );
    rendered(&reply)
}

/// RULE: every call that moves an application's own `Status` refuses an application whose
/// registration settled as FAILED with `ValidationException` (400) naming it, identically,
/// however often it is asked — filing no work and changing nothing — while the same call
/// against an application whose registration succeeded is served.
#[tokio::test]
async fn every_call_that_moves_an_application_status_refuses_a_failed_registration() {
    let emulator = WithEc2::new();
    // One held instance per control, so the row that asks for the Amazon EC2 shutdown
    // reaches an instance of its own.
    let launched = emulator
        .launch_running(i32::try_from(STATUS_MOVES.len()).expect("a small row count"))
        .await;

    // Every registration is filed before any of them is waited for, so the rows' clocks
    // run together rather than one after another. Naming an instance the owner holds no
    // record of is what the discovery cannot be carried out against.
    for application_id in FAILED_IDS {
        register(&emulator, application_id, UNHELD_INSTANCE).await;
    }
    for (index, row) in STATUS_MOVES.iter().enumerate() {
        register(&emulator, row.control_id, &launched[index]).await;
    }

    for application_id in FAILED_IDS {
        wait_until(&emulator, application_id, application_id, "FAILED").await;

        // What the application reports before any of the class is asked of it.
        let before = reported(&emulator, application_id).await;

        // Every refusal, reduced to what a client acts on: the status, the error code and
        // the sentence. One distinct answer for the whole class is the rule.
        let mut answers: BTreeSet<(u16, String, String)> = BTreeSet::new();
        for row in STATUS_MOVES {
            for attempt in 1..=ATTEMPTS {
                let case = format!(
                    "{} {} against {application_id:?}, attempt {attempt}",
                    row.path, row.switches
                );
                let refused = emulator.reply(row.path, request(row, application_id)).await;
                assert_eq!(
                    (refused.status, refused.code()),
                    (400, VALIDATION_EXCEPTION),
                    "case {case:?}: expected 400 {VALIDATION_EXCEPTION}, got {refused:?}"
                );
                assert_eq!(
                    message(&refused),
                    failed_registration(application_id),
                    "case {case:?}: expected the failed-registration refusal, got {:?}",
                    message(&refused)
                );
                assert!(
                    refused.member("OperationId").is_none(),
                    "case {case:?}: a refused call hands out no OperationId, got {refused:?}"
                );
                answers.insert((
                    refused.status,
                    refused.code().to_string(),
                    message(&refused),
                ));
            }
        }
        assert_eq!(
            answers.len(),
            1,
            "case {application_id:?}: every call of the class is refused the one way, got \
             {answers:?}"
        );

        // The refusals changed nothing: the application reports exactly what it did, and
        // no record of either call was filed against it.
        let after = reported(&emulator, application_id).await;
        assert_eq!(
            after, before,
            "case {application_id:?}: a refused status move leaves the application as it \
             was, got {after}"
        );
        for op_type in ["START_APPLICATION", "STOP_APPLICATION"] {
            let filed = records_of_type(&emulator, application_id, op_type).await;
            assert!(
                filed.is_empty(),
                "case {application_id:?}: a refused status move files no {op_type} record, \
                 got {filed:?}"
            );
        }
    }

    // The SAME calls against an application whose registration SUCCEEDED are served, so
    // the refusal above is about the registration outcome and not about the call.
    for row in STATUS_MOVES {
        let case = format!(
            "{} {} against a discovered application",
            row.path, row.switches
        );
        wait_until(&emulator, &case, row.control_id, "ACTIVATED").await;
        let served = emulator.reply(row.path, request(row, row.control_id)).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: the same call is served for a discovered application, got \
             {served:?}"
        );
        assert!(
            served.member("OperationId").is_some(),
            "case {case:?}: a served call files the record of its own work, got {served:?}"
        );
    }
}
