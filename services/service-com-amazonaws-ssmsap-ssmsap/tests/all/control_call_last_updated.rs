// SPDX-License-Identifier: CC0-1.0

//! When an application's `LastUpdated` changes, driven over the wire.
//!
//! RULE under test: a control call this service ACCEPTS on a registered application
//! changes that application's record at the instant it is accepted — the asynchronous
//! work it filed is part of the record — so `LastUpdated` is that instant, whatever else
//! the call moves about the application, and when the filed work falls due `LastUpdated`
//! advances again to that due instant. A call the service REFUSES changes nothing, so it
//! leaves `LastUpdated` exactly as it was.
//!
//! Every case goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state`: when the record changed is
//! only believed when `GetApplication`, `GetComponent` and `GetDatabase` report it back.
//!
//! The two instants are read off the record the call itself filed rather than off the host
//! clock: `Operation$StartTime` is the instant the accepted call was made and
//! `Operation$EndTime` the instant its work fell due, so each row compares two values the
//! emulator computed and pins nothing about how fast the host runs.
//!
//! The rows span what the rule generalises over: which control call was accepted (start,
//! stop, re-discovery, a settings update), what it moves about the application (its
//! `Status`, its `DiscoveryStatus`, neither), and which settings members an update carries
//! (credentials added, a secret replaced, an add beside a removal, the Backint agent
//! configuration alone, the SAP HANA connection alone, every setting at once).

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::BACKINT_MODE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    application_not_found_message, MISSING_SETTINGS_MESSAGE, TRANSITION_SPAN,
};

use crate::application_lifecycle::{string_list, valid_registration, Emulator, Reply};
use crate::list_databases::expected_arn as database_arn;

/// `OperationStatus$INPROGRESS`, as the model spells it: the status the record of an
/// accepted call carries until the clock brings its outcome due.
const OPERATION_STATUS_INPROGRESS: &str = "INPROGRESS";

/// The single `CredentialType` the model defines
/// (`service_com_amazonaws_ssmsap_ssmsap::ops::types::CREDENTIAL_TYPE_VALUES`).
const ADMIN: &str = "ADMIN";

/// The `DatabaseName` of the credential every registration here carries — the SID a
/// `valid_registration` names, so it is the database of the application's HANA component.
const REGISTERED_DATABASE: &str = "HDB";

/// How long a condition the clock owes an application is given to come due.
///
/// Generous on purpose: nothing in the emulator sleeps for a due instant, so every wait
/// below polls for the CONDITION and this is only the deadline that stops a broken build
/// polling forever. It pins nothing about how fast the host runs.
fn settle_budget() -> Duration {
    TRANSITION_SPAN + Duration::from_secs(60)
}

/// How long each poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One credential declaration as the wire carries it.
fn credential(database_name: &str, secret_id: &str) -> Value {
    json!([{
        "DatabaseName": database_name,
        "CredentialType": ADMIN,
        "SecretId": secret_id,
    }])
}

/// One accepted control call: the row of the table.
struct Accepted {
    /// How an assertion names the row.
    name: &'static str,
    /// The operation's HTTP path, as the model binds it.
    path: &'static str,
    /// The request body, for the application the row registered.
    body: fn(&str) -> Value,
}

/// The `UpdateApplicationSettings` body naming `members`, for `id`.
fn settings_update(id: &str, members: &[(&str, Value)]) -> Value {
    let mut body = Map::new();
    body.insert("ApplicationId".into(), json!(id));
    for (member, value) in members {
        body.insert((*member).into(), value.clone());
    }
    Value::Object(body)
}

/// The `Backint` configuration an update installs: the model's single `BackintMode`
/// value, and the switch that asks the service to refuse while a backup is running.
fn backint() -> Value {
    json!({
        "BackintMode": BACKINT_MODE_VALUES[0],
        "EnsureNoBackupInProcess": true,
    })
}

/// The rows: every control call of this service that files asynchronous work on an
/// application it already holds.
const ACCEPTED: &[Accepted] = &[
    // Moves the application's own `Status`.
    Accepted {
        name: "a start of a settled application",
        path: "/start-application",
        body: |id| json!({"ApplicationId": id}),
    },
    Accepted {
        name: "a stop of a settled application",
        path: "/stop-application",
        body: |id| json!({"ApplicationId": id}),
    },
    // Moves its `DiscoveryStatus` alone.
    Accepted {
        name: "a re-discovery of a settled application",
        path: "/start-application-refresh",
        body: |id| json!({"ApplicationId": id}),
    },
    // Moves neither: a settings update files what the application's settings BECOME.
    Accepted {
        name: "an update adding a credential for another database",
        path: "/update-application-settings",
        body: |id| {
            settings_update(
                id,
                &[(
                    "CredentialsToAddOrUpdate",
                    credential("TNT1", "sap/tnt1/admin"),
                )],
            )
        },
    },
    Accepted {
        name: "an update replacing the secret of a credential already held",
        path: "/update-application-settings",
        body: |id| {
            settings_update(
                id,
                &[(
                    "CredentialsToAddOrUpdate",
                    credential(REGISTERED_DATABASE, "sap/hdb/rotated"),
                )],
            )
        },
    },
    Accepted {
        name: "an update adding one credential and removing another",
        path: "/update-application-settings",
        body: |id| {
            settings_update(
                id,
                &[
                    (
                        "CredentialsToAddOrUpdate",
                        credential("TNT1", "sap/tnt1/admin"),
                    ),
                    (
                        "CredentialsToRemove",
                        credential(REGISTERED_DATABASE, "sap/hdb/admin"),
                    ),
                ],
            )
        },
    },
    Accepted {
        name: "an update carrying the Backint agent configuration alone",
        path: "/update-application-settings",
        body: |id| settings_update(id, &[("Backint", backint())]),
    },
    Accepted {
        name: "an update carrying the SAP HANA connection alone",
        path: "/update-application-settings",
        body: |id| {
            settings_update(
                id,
                &[(
                    "DatabaseArn",
                    json!(database_arn(id, REGISTERED_DATABASE)),
                )],
            )
        },
    },
    Accepted {
        name: "an update carrying every setting at once",
        path: "/update-application-settings",
        body: |id| {
            settings_update(
                id,
                &[
                    (
                        "CredentialsToAddOrUpdate",
                        credential("TNT2", "sap/tnt2/admin"),
                    ),
                    ("Backint", backint()),
                    (
                        "DatabaseArn",
                        json!(database_arn(id, REGISTERED_DATABASE)),
                    ),
                ],
            )
        },
    },
];

/// `GetApplication` for `id`.
async fn get_application(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .send("/get-application", json!({"ApplicationId": id}))
        .await
}

/// The `Application$LastUpdated` of `id`, as restJson1 serializes an epoch timestamp.
async fn last_updated(emulator: &Emulator, case: &str, id: &str) -> Value {
    let reply = get_application(emulator, id).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetApplication, got {reply:?}"
    );
    reply
        .app("LastUpdated")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Application LastUpdated, got {reply:?}"))
}

/// Register a HANA application under `id` and wait until the clock owes its registration
/// nothing — the application is up, its discovery succeeded, and no work is in flight on
/// it, so the next control call is the only thing that can change its record.
async fn register_settled(emulator: &Emulator, case: &str, id: &str) {
    let reply = emulator
        .send("/register-application", valid_registration(id, "HANA"))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterApplication, got {reply:?}"
    );
    let deadline = Instant::now() + settle_budget();
    loop {
        let application = get_application(emulator, id).await;
        let status = application.app_str("Status").to_string();
        let discovery_status = application.app_str("DiscoveryStatus").to_string();
        if status == "ACTIVATED" && discovery_status == "SUCCESS" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the registration of {id} must settle, it reports \
             {status:?}/{discovery_status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The id of the ONE operation record a control call answered with, whichever member the
/// operation models for it (`OperationId` for a start, a stop and a refresh,
/// `OperationIds` for a settings update).
fn filed_operation_id(case: &str, reply: &Reply) -> String {
    if let Some(id) = reply.member("OperationId").and_then(Value::as_str) {
        return id.to_string();
    }
    let ids = reply
        .member("OperationIds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        ids.len(),
        1,
        "case {case:?}: one call files one operation record, got {reply:?}"
    );
    ids[0].as_str().unwrap_or_default().to_string()
}

/// `GetOperation` for `operation_id`.
async fn get_operation(emulator: &Emulator, case: &str, operation_id: &str) -> Reply {
    let reply = emulator
        .send("/get-operation", json!({"OperationId": operation_id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetOperation, got {reply:?}"
    );
    reply
}

/// The `Operation$StartTime` of the record `operation_id`: the instant the accepted call
/// was made, as the emulator computed it.
async fn operation_start_time(emulator: &Emulator, case: &str, operation_id: &str) -> Value {
    let reply = get_operation(emulator, case, operation_id).await;
    reply
        .op("StartTime")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Operation StartTime, got {reply:?}"))
}

/// Wait until the record `operation_id` has reached its terminal status and answer its
/// `Operation$EndTime`: the instant the work the call filed fell due.
///
/// Polls the condition rather than a duration: the outcome is due one `TRANSITION_SPAN`
/// after the call whether or not anybody asks, so this looks again until the record
/// reports it.
async fn settled_end_time(emulator: &Emulator, case: &str, operation_id: &str) -> Value {
    let deadline = Instant::now() + settle_budget();
    loop {
        let reply = get_operation(emulator, case, operation_id).await;
        let status = reply.op_str("Status").to_string();
        if status != OPERATION_STATUS_INPROGRESS {
            return reply.op("EndTime").cloned().unwrap_or_else(|| {
                panic!("case {case:?}: a finished operation has an EndTime, got {reply:?}")
            });
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the record {operation_id} must settle, it reports {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: an accepted control call changes the application's record at the instant it is
/// accepted, so `LastUpdated` is that instant — the `StartTime` of the record the call
/// filed — whatever the call moves about the application.
#[tokio::test]
async fn an_accepted_control_call_stamps_last_updated_with_the_instant_of_the_call() {
    for case in ACCEPTED {
        let emulator = Emulator::new();
        let id = "hdb-stamp";
        register_settled(&emulator, case.name, id).await;
        let registered_at = last_updated(&emulator, case.name, id).await;

        let accepted = emulator.send(case.path, (case.body)(id)).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected 200 from {}, got {accepted:?}",
            case.name, case.path
        );
        let operation_id = filed_operation_id(case.name, &accepted);
        let called_at = operation_start_time(&emulator, case.name, &operation_id).await;
        let stamped = last_updated(&emulator, case.name, id).await;
        assert_eq!(
            stamped, called_at,
            "case {:?}: LastUpdated is the instant of the accepted call ({called_at}), got \
             {stamped}",
            case.name
        );
        assert_ne!(
            stamped, registered_at,
            "case {:?}: an accepted call leaves the record changed, it still reports \
             {registered_at}",
            case.name
        );
    }
}

/// RULE: when the work an accepted control call filed falls due, `LastUpdated` advances
/// again to that due instant — the `EndTime` of the record the call filed.
#[tokio::test]
async fn the_work_a_control_call_filed_advances_last_updated_to_its_due_instant() {
    for case in ACCEPTED {
        let emulator = Emulator::new();
        let id = "hdb-due";
        register_settled(&emulator, case.name, id).await;

        let accepted = emulator.send(case.path, (case.body)(id)).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected 200 from {}, got {accepted:?}",
            case.name, case.path
        );
        let operation_id = filed_operation_id(case.name, &accepted);
        let called_at = operation_start_time(&emulator, case.name, &operation_id).await;
        let due_at = settled_end_time(&emulator, case.name, &operation_id).await;
        let settled = last_updated(&emulator, case.name, id).await;
        assert_eq!(
            settled, due_at,
            "case {:?}: LastUpdated is the instant the filed work fell due ({due_at}), got \
             {settled}",
            case.name
        );
        assert_ne!(
            settled, called_at,
            "case {:?}: the due instant is later than the call that filed the work, both \
             report {called_at}",
            case.name
        );
    }
}

/// RULE: `Component$LastUpdated` and `Database$LastUpdated` are projected from the same
/// instant the application's record last changed, so an accepted call that changes that
/// record changes what all three readers report.
#[tokio::test]
async fn every_reader_of_the_record_reports_the_instant_of_the_accepted_call() {
    for case in ACCEPTED {
        let emulator = Emulator::new();
        let id = "hdb-projected";
        register_settled(&emulator, case.name, id).await;

        let accepted = emulator.send(case.path, (case.body)(id)).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected 200 from {}, got {accepted:?}",
            case.name, case.path
        );
        let operation_id = filed_operation_id(case.name, &accepted);
        let called_at = operation_start_time(&emulator, case.name, &operation_id).await;

        let application = get_application(&emulator, id).await;
        let components = string_list(application.app("Components"));
        assert!(
            !components.is_empty(),
            "case {:?}: a discovered application has components, got {application:?}",
            case.name
        );
        for component_id in &components {
            let component = emulator
                .send(
                    "/get-component",
                    json!({"ApplicationId": id, "ComponentId": component_id}),
                )
                .await;
            assert_eq!(
                component.status, 200,
                "case {:?}: expected 200 from GetComponent for {component_id}, got \
                 {component:?}",
                case.name
            );
            let reported = component
                .member("Component")
                .and_then(|value| value.get("LastUpdated"))
                .cloned();
            assert_eq!(
                reported.as_ref(),
                Some(&called_at),
                "case {:?}: Component {component_id} reports the instant the record \
                 changed ({called_at}), got {reported:?}",
                case.name
            );
        }

        let database = emulator
            .send(
                "/get-database",
                json!({"ApplicationId": id, "DatabaseId": REGISTERED_DATABASE}),
            )
            .await;
        assert_eq!(
            database.status, 200,
            "case {:?}: expected 200 from GetDatabase, got {database:?}",
            case.name
        );
        let reported = database
            .member("Database")
            .and_then(|value| value.get("LastUpdated"))
            .cloned();
        assert_eq!(
            reported.as_ref(),
            Some(&called_at),
            "case {:?}: Database {REGISTERED_DATABASE} reports the instant the record \
             changed ({called_at}), got {reported:?}",
            case.name
        );
    }
}

/// One control call the service REFUSES, and the status it answers with.
struct Refused {
    name: &'static str,
    path: &'static str,
    body: fn(&str) -> Value,
    status: u16,
}

/// The rows: a refusal from each channel a control call of this service refuses on — a
/// request that asks for no change, one that asks for a change the application cannot
/// take, and one that names an application this Region holds none of.
const REFUSED: &[Refused] = &[
    Refused {
        name: "an update naming no setting at all",
        path: "/update-application-settings",
        body: |id| json!({"ApplicationId": id}),
        status: 400,
    },
    Refused {
        name: "an update removing a credential the application does not hold",
        path: "/update-application-settings",
        body: |id| {
            settings_update(
                id,
                &[("CredentialsToRemove", credential("TNT9", "sap/tnt9/admin"))],
            )
        },
        status: 400,
    },
    Refused {
        name: "an update that would leave a HANA application without credentials",
        path: "/update-application-settings",
        body: |id| {
            settings_update(
                id,
                &[(
                    "CredentialsToRemove",
                    credential(REGISTERED_DATABASE, "sap/hdb/admin"),
                )],
            )
        },
        status: 400,
    },
    Refused {
        name: "a start naming an application this Region holds none of",
        path: "/start-application",
        body: |_| json!({"ApplicationId": "no-such-application"}),
        status: 404,
    },
];

/// RULE: a call the service refuses changes nothing, so it leaves `LastUpdated` exactly as
/// it was — only an ACCEPTED call stamps it.
#[tokio::test]
async fn a_refused_control_call_leaves_last_updated_as_it_was() {
    for case in REFUSED {
        let emulator = Emulator::new();
        let id = "hdb-refused";
        register_settled(&emulator, case.name, id).await;
        let before = last_updated(&emulator, case.name, id).await;

        let refused = emulator.send(case.path, (case.body)(id)).await;
        assert_eq!(
            refused.status, case.status,
            "case {:?}: expected {} from {}, got {refused:?}",
            case.name, case.status, case.path
        );
        let after = last_updated(&emulator, case.name, id).await;
        assert_eq!(
            after, before,
            "case {:?}: a refused call changes nothing, expected LastUpdated {before}, got \
             {after}",
            case.name
        );
    }
}

/// RULE: a control call refused because work is already in flight on the application
/// leaves the stamp of the call that filed that work — the refusal of a second call is not
/// a change to the record either.
#[tokio::test]
async fn a_call_refused_while_work_is_in_flight_keeps_the_stamp_of_the_accepted_call() {
    let case = "a start refused while a settings update is in flight";
    let emulator = Emulator::new();
    let id = "hdb-in-flight";
    register_settled(&emulator, case, id).await;

    let accepted = emulator
        .send(
            "/update-application-settings",
            settings_update(
                id,
                &[(
                    "CredentialsToAddOrUpdate",
                    credential("TNT1", "sap/tnt1/admin"),
                )],
            ),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: expected 200 from UpdateApplicationSettings, got {accepted:?}"
    );
    let stamped = last_updated(&emulator, case, id).await;

    let refused = emulator
        .send("/start-application", json!({"ApplicationId": id}))
        .await;
    assert_eq!(
        refused.status, 409,
        "case {case:?}: a control call is refused while work is in flight, got {refused:?}"
    );
    let after = last_updated(&emulator, case, id).await;
    assert_eq!(
        after, stamped,
        "case {case:?}: the refusal changes nothing, expected LastUpdated {stamped}, got \
         {after}"
    );
}

/// The refusal wording the rows above are answered with comes from the one place this
/// service words it, so a row that drives a channel reads the same sentence a client does.
#[tokio::test]
async fn the_refusals_the_rows_drive_are_the_services_own_wording() {
    let emulator = Emulator::new();
    let case = "the refusal wording of a refused control call";
    let id = "hdb-wording";
    register_settled(&emulator, case, id).await;

    let no_setting = emulator
        .send("/update-application-settings", json!({"ApplicationId": id}))
        .await;
    let message = no_setting
        .member("message")
        .or_else(|| no_setting.member("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        message, MISSING_SETTINGS_MESSAGE,
        "case {case:?}: expected the service's own wording, got {no_setting:?}"
    );

    let unknown = emulator
        .send(
            "/start-application",
            json!({"ApplicationId": "no-such-application"}),
        )
        .await;
    let message = unknown
        .member("message")
        .or_else(|| unknown.member("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        message,
        application_not_found_message("no-such-application"),
        "case {case:?}: expected the service's own wording, got {unknown:?}"
    );
}
