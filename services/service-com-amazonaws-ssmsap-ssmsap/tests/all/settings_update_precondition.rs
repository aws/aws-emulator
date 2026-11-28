// SPDX-License-Identifier: CC0-1.0

//! What `UpdateApplicationSettings` answers for an application the service never
//! DISCOVERED.
//!
//! Every case goes through the real router with the owner of the Amazon EC2 instances
//! mounted beside this service (the harness `tests/all/ec2_instance_shutdown.rs` owns, built
//! on the one `tests/all/application_lifecycle.rs` every suite of this target drives), so the
//! restJson1 routing, the boundary constraint walk, the discovery hop into EC2, the handler
//! and the response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state`: what an update did or did not change is only believed when
//! `ListDatabases` and `ListOperations` report it back.
//!
//! RULE under test: the settings of an application are the settings of the SAP system its
//! registration established, so a call that updates them requires a registration that
//! COMPLETED successfully — one that failed discovered nothing, so there is no SAP system for
//! credentials, an AWS Backint Agent configuration or a replaced SAP HANA connection to
//! apply to. Such a call is refused with the modelled `ConflictException` (HTTP 409) in this
//! service's fixed wording for that precondition, which names nothing of the request, and it
//! writes NOTHING: no operation record is filed and no SAP HANA database is derived from the
//! credentials it declared. The same update against an application whose registration
//! completed successfully is served, so the refusal is about the registration outcome and not
//! about the request.
//!
//! The rows span what that rule generalises over: WHICH settings member the update names
//! (a credential added, the registered credential removed, the Backint agent configured, the
//! SAP HANA connection replaced, and every member at once), the `ApplicationType`, and the
//! forms the modelled `ApplicationId` admits.
//!
//! Nothing here waits a fixed time: every span this suite crosses is a stored due instant and
//! nothing in the emulator sleeps for one, so a case LOOKS AGAIN until the condition holds or
//! [`SETTLE_DEADLINE`] passes, and no assertion is made about elapsed time.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::BACKINT_MODE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    ABAP_APPLICATION_TYPE, APPLICATION_STATUS_ACTIVATED, APPLICATION_STATUS_FAILED,
    HANA_APPLICATION_TYPE,
};

use crate::application_lifecycle::{valid_registration, Reply, REGION};
use crate::database_status_discovery::wait_until;
use crate::ec2_instance_shutdown::{registration, Emulator as WithEc2, UNHELD_INSTANCE};
use crate::registration_outcome::records_of_type;
use crate::resource_tags::database_arn;
use crate::validation_message_rendering::message;

/// The modelled error code a call that cannot act on the application in the state it is in
/// is refused with (`smithy_query(query="operation",
/// operation="UpdateApplicationSettings")`: `ConflictException` `@httpError 409`).
const CONFLICT_EXCEPTION: &str = "ConflictException";

/// How this service states that the application a call names has not been registered
/// successfully: entirely fixed text, naming nothing of the request.
///
/// Spelled out here rather than taken from the crate, so the wording is pinned
/// independently of the one place the handlers build it in.
const NOT_REGISTERED_SUCCESSFULLY: &str =
    "The specified application has not been registered successfully";

/// The `OperationType` of the record an accepted `UpdateApplicationSettings` files, as
/// `ListOperations` reports it in `Operation$Type`.
const UPDATE_APPLICATION_SETTINGS: &str = "UPDATE_APPLICATION_SETTINGS";

/// The `DatabaseName` the shared registration builder's credential declares, and therefore
/// the credential a row can ask to have REMOVED.
const REGISTERED_DATABASE_NAME: &str = "HDB";

/// The `DatabaseName` of a SAP HANA database no registration here names, so a credential
/// declared for it is one the application holds none for — and a database that would be
/// derived from it the instant such a credential were accepted.
const ADDED_DATABASE_NAME: &str = "TNT1";

/// How long a case is given for an outcome another clock owes.
///
/// Generous on purpose: the outcome is a stored due instant and nothing sleeps for one, so a
/// case looks again until it is reported. All this bound is for is that an outcome which never
/// comes fails instead of hanging; nothing asserts on elapsed time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long one look-again cycle pauses before reading again. Not a bound on anything.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One settings change a request can ask for: the members `UpdateApplicationSettings` takes
/// beside the `@required` `ApplicationId` (`smithy_query(query="operation",
/// operation="UpdateApplicationSettings")`: `Backint`, `CredentialsToAddOrUpdate`,
/// `CredentialsToRemove`, `DatabaseArn`).
#[derive(Debug, Clone, Copy)]
enum Setting {
    /// A credential for a SAP HANA database the application holds none for.
    AddCredential,
    /// The credential the registration itself declared.
    RemoveCredential,
    /// An AWS Backint Agent for SAP HANA configuration.
    Backint,
    /// A SAP HANA database that replaces the application's own connection.
    ConnectDatabase,
    /// Every one of them in a single call.
    Everything,
}

impl Setting {
    /// The members this change asks for, written into `body`.
    ///
    /// `connection` is the ARN of the SAP HANA database a replaced connection names.
    fn write(self, body: &mut Map<String, Value>, connection: &str) {
        match self {
            Self::AddCredential => {
                body.insert(
                    "CredentialsToAddOrUpdate".into(),
                    credential(ADDED_DATABASE_NAME),
                );
            }
            Self::RemoveCredential => {
                body.insert(
                    "CredentialsToRemove".into(),
                    credential(REGISTERED_DATABASE_NAME),
                );
            }
            Self::Backint => {
                body.insert(
                    "Backint".into(),
                    json!({
                        // The model's own `BackintMode` catalogue, so the row carries a
                        // value the API defines rather than one spelled here.
                        "BackintMode": BACKINT_MODE_VALUES[0],
                        "EnsureNoBackupInProcess": false,
                    }),
                );
            }
            Self::ConnectDatabase => {
                body.insert("DatabaseArn".into(), json!(connection));
            }
            Self::Everything => {
                for setting in [
                    Self::AddCredential,
                    Self::RemoveCredential,
                    Self::Backint,
                    Self::ConnectDatabase,
                ] {
                    setting.write(body, connection);
                }
            }
        }
    }
}

/// A one-element `ApplicationCredentialList` naming the `ADMIN` credential of the SAP HANA
/// database `database_name` (`ApplicationCredential` requires `DatabaseName`,
/// `CredentialType` — an enum whose value is `ADMIN` — and `SecretId`).
fn credential(database_name: &str) -> Value {
    json!([{
        "DatabaseName": database_name,
        "CredentialType": "ADMIN",
        "SecretId": format!("sap/{}/admin", database_name.to_lowercase()),
    }])
}

/// The `UpdateApplicationSettings` body asking for `setting` against `application_id`.
fn update_request(application_id: &str, setting: Setting, connection: &str) -> Value {
    let mut body = Map::new();
    body.insert("ApplicationId".into(), json!(application_id));
    setting.write(&mut body, connection);
    Value::Object(body)
}

/// One application whose registration FAILED, and the settings change asked of it.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The `ApplicationId`, spanning the forms the modelled `^[\w\d\.-]+$` admits.
    application_id: &'static str,
    /// An `ApplicationType` value (`smithy_query(query="shape", shape="ApplicationType")`:
    /// `HANA` | `SAP_ABAP`).
    application_type: &'static str,
    setting: Setting,
}

/// The table: each settings member the call takes, asked of a registration that failed.
const ROWS: &[Row] = &[
    Row {
        case: "a credential for a database it holds none for",
        application_id: "settings-failed-credential",
        application_type: HANA_APPLICATION_TYPE,
        setting: Setting::AddCredential,
    },
    Row {
        case: "the credential its registration declared, removed",
        application_id: "SETTINGS.FAILED-2",
        application_type: HANA_APPLICATION_TYPE,
        setting: Setting::RemoveCredential,
    },
    Row {
        case: "the Backint agent configured",
        application_id: "settings_failed_3",
        application_type: HANA_APPLICATION_TYPE,
        setting: Setting::Backint,
    },
    Row {
        case: "its SAP HANA connection replaced",
        application_id: "s",
        application_type: HANA_APPLICATION_TYPE,
        setting: Setting::ConnectDatabase,
    },
    Row {
        case: "every settings member in one call",
        application_id: "settings-failed-everything",
        application_type: HANA_APPLICATION_TYPE,
        setting: Setting::Everything,
    },
    Row {
        case: "an ABAP application's SAP HANA connection replaced",
        application_id: "abap.settings-failed",
        application_type: ABAP_APPLICATION_TYPE,
        setting: Setting::ConnectDatabase,
    },
    Row {
        case: "a credential for an ABAP application",
        application_id: "abap-settings-failed-2",
        application_type: ABAP_APPLICATION_TYPE,
        setting: Setting::AddCredential,
    },
];

/// The `RegisterApplication` body for an application of `application_type` running on
/// `instance`.
///
/// A HANA registration goes through the builder this target's EC2 harness shares (which
/// supplies the SID, instance number and credentials that type requires); an ABAP
/// registration carries what ITS type requires, which is no credential at all
/// (`service_com_amazonaws_ssmsap_ssmsap::state::CONDITIONALLY_REQUIRED_MEMBERS`).
fn registration_of(application_id: &str, application_type: &str, instance: &str) -> Value {
    if application_type == HANA_APPLICATION_TYPE {
        return registration(application_id, instance, &[]);
    }
    let mut body = valid_registration(application_id, application_type);
    body["Instances"] = json!([instance]);
    body
}

/// Register `application_id` of `application_type` on `instance`, with no wait.
async fn register(
    emulator: &WithEc2,
    case: &str,
    application_id: &str,
    application_type: &str,
    instance: &str,
) {
    let reply = emulator
        .reply(
            "/register-application",
            registration_of(application_id, application_type, instance),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterApplication for {application_id:?}, got \
         {reply:?}"
    );
}

/// The `DatabaseId` of every SAP HANA database `ListDatabases` reports for `application_id`.
async fn database_ids(emulator: &WithEc2, case: &str, application_id: &str) -> BTreeSet<String> {
    let reply = emulator
        .reply("/list-databases", json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListDatabases for {application_id:?}, got {reply:?}"
    );
    reply
        .member("Databases")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("DatabaseId").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Ask for `setting` against `application_id` and answer what the wire said.
async fn update(
    emulator: &WithEc2,
    application_id: &str,
    setting: Setting,
    connection: &str,
) -> Reply {
    emulator
        .reply(
            "/update-application-settings",
            update_request(application_id, setting, connection),
        )
        .await
}

/// Assert that `application_id` holds exactly the databases `expected` and has no
/// `UPDATE_APPLICATION_SETTINGS` record filed against it — what a refused update leaves
/// behind, which is nothing.
async fn nothing_was_applied(
    emulator: &WithEc2,
    case: &str,
    application_id: &str,
    expected: &BTreeSet<String>,
    when: &str,
) {
    let filed = records_of_type(emulator, application_id, UPDATE_APPLICATION_SETTINGS).await;
    assert!(
        filed.is_empty(),
        "case {case:?} {when}: a refused update files no record, got {filed:?}"
    );
    let databases = database_ids(emulator, case, application_id).await;
    assert_eq!(
        &databases, expected,
        "case {case:?} {when}: a refused update derives no database, expected {expected:?}, \
         got {databases:?}"
    );
}

/// Look again until `application_id` reports a database whose `DatabaseId` is
/// `database_id` — the settings an accepted update leaves outstanding, which the clock owes
/// the application.
async fn wait_for_database(emulator: &WithEc2, case: &str, application_id: &str, database_id: &str) {
    let until = Instant::now() + SETTLE_DEADLINE;
    loop {
        let databases = database_ids(emulator, case, application_id).await;
        if databases.contains(database_id) {
            return;
        }
        assert!(
            Instant::now() < until,
            "case {case:?}: expected the accepted update to derive database {database_id:?}, \
             {application_id:?} reports {databases:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: a settings update requires a registration that completed successfully, so one
/// whose registration FAILED is refused with `ConflictException` (409) in the fixed wording
/// and nothing of the update is applied — while the same update against a discovered
/// application is served and materialises.
#[tokio::test]
async fn a_settings_update_needs_a_registration_that_completed_successfully() {
    let emulator = WithEc2::new();
    // One instance the owner of the account's instances holds, for the applications whose
    // discovery has to be able to run.
    let held = emulator.launch_running(1).await[0].clone();

    // The application whose SAP HANA database a replaced connection names, so a row names a
    // database of a discovered application rather than one nothing holds.
    let partner = "settings-connection-source";
    // The control: the SAME update, against an application the service discovered.
    let control = "settings-discovered";

    // Every registration is filed first and waited for afterwards: each settles on its own
    // stored due instant, so the waits overlap instead of adding up.
    register(&emulator, partner, partner, HANA_APPLICATION_TYPE, &held).await;
    register(&emulator, control, control, HANA_APPLICATION_TYPE, &held).await;
    for row in ROWS {
        register(
            &emulator,
            row.case,
            row.application_id,
            row.application_type,
            UNHELD_INSTANCE,
        )
        .await;
    }
    wait_until(&emulator, partner, partner, APPLICATION_STATUS_ACTIVATED).await;
    wait_until(&emulator, control, control, APPLICATION_STATUS_ACTIVATED).await;
    for row in ROWS {
        wait_until(
            &emulator,
            row.case,
            row.application_id,
            APPLICATION_STATUS_FAILED,
        )
        .await;
    }

    // The ARN of the partner's own SAP HANA database, which is the connection a row replaces
    // the application's with. A `DatabaseId` is the canonical form of the database's name,
    // which for this name is the name itself.
    let connection = database_arn(REGION, partner, REGISTERED_DATABASE_NAME);

    for row in ROWS {
        let case = row.case;
        let application_id = row.application_id;
        // What the FAILED registration itself derived, which the refusal must leave exactly
        // as it is.
        let before = database_ids(&emulator, case, application_id).await;

        let reply = update(&emulator, application_id, row.setting, &connection).await;
        assert_eq!(
            (reply.status, reply.code()),
            (409, CONFLICT_EXCEPTION),
            "case {case:?}: expected 409 {CONFLICT_EXCEPTION} for a registration that failed, \
             got {reply:?}"
        );
        assert_eq!(
            message(&reply),
            NOT_REGISTERED_SUCCESSFULLY,
            "case {case:?}: expected the fixed precondition wording, got {:?}",
            message(&reply)
        );
        assert!(
            reply.member("OperationIds").is_none(),
            "case {case:?}: a refused update hands out no operation, got {reply:?}"
        );
        nothing_was_applied(&emulator, case, application_id, &before, "at once").await;
        // Still FAILED: the refusal moved nothing.
        wait_until(&emulator, case, application_id, APPLICATION_STATUS_FAILED).await;
    }

    // The control, asked for every settings member at once AFTER the refusals above: it is
    // served, and the credential it adds becomes a database of the application.
    let case = "an application the service discovered";
    let served = update(&emulator, control, Setting::Everything, &connection).await;
    assert_eq!(
        served.status, 200,
        "case {case:?}: expected 200 for a registration that completed successfully, got \
         {served:?}"
    );
    assert!(
        served.member("Message").is_some(),
        "case {case:?}: a served update reports what it is updating, got {served:?}"
    );
    let operations = served
        .member("OperationIds")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        operations, 1,
        "case {case:?}: a served update files one operation, got {served:?}"
    );
    let filed = records_of_type(&emulator, control, UPDATE_APPLICATION_SETTINGS).await;
    assert_eq!(
        filed.len(),
        1,
        "case {case:?}: the served update's record is readable, got {filed:?}"
    );
    wait_for_database(&emulator, case, control, ADDED_DATABASE_NAME).await;

    // The control's update was asked for after every refused one and has now settled, so
    // anything a refused update had left outstanding would have been due as well: each
    // refused application still holds exactly what its failed registration derived.
    for row in ROWS {
        let before = if row.application_type == HANA_APPLICATION_TYPE {
            // A HANA registration declares one credential, which derives one database.
            BTreeSet::from([REGISTERED_DATABASE_NAME.to_string()])
        } else {
            // An ABAP registration declares no credential and names no database.
            BTreeSet::new()
        };
        nothing_was_applied(
            &emulator,
            row.case,
            row.application_id,
            &before,
            "once a later update has settled",
        )
        .await;
    }
}
