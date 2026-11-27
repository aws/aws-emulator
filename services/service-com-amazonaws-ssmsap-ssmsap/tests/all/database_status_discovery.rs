// SPDX-License-Identifier: CC0-1.0

//! What `Database$Status` reports for an application the service never DISCOVERED.
//!
//! Every case goes through the real router with the owner of the Amazon EC2 instances
//! mounted beside this service (the harness `tests/all/ec2_instance_shutdown.rs` owns, built
//! on the one `tests/all/application_lifecycle.rs` every suite of this target drives), so the
//! restJson1 routing, the boundary constraint walk, the discovery hop into EC2, the handler
//! and the response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state`: a status is only believed when `GetDatabase` reports it back.
//!
//! RULE under test: a database lives in the database management system its application is
//! connected to, so the status it reports is that DBMS's — but only where the service has
//! ESTABLISHED what the application runs, which is where its discovery completed
//! successfully and the application is in a status that states a running state. Where the
//! service established nothing (the registration is still under way, or its discovery could
//! not be carried out) nothing is known about that application's databases whatever the DBMS
//! record says, which is what the model's `UNKNOWN` states
//! (`smithy_query(query="shape", shape="DatabaseStatus")` is the catalogue every row is
//! checked against). The application's own `Status` alone is not that evidence: a stop is
//! served against an application whose registration is still running, so a never-discovered
//! application reaches the transient and settled statuses of a stop too.
//!
//! The rows span what that rule generalises over: whether the owner of the named instance
//! holds it (so whether discovery can be carried out at all) and in which of the two ways it
//! resolves to no instance; which status the application is read in (registering, settled
//! failed, stopping, stopped, activated); whether a stop named the connected DBMS, so what
//! the DBMS record itself says (running, stopping, stopped); and how many databases the
//! registration derives.
//!
//! Nothing here waits a fixed time: every span this suite crosses is a stored due instant and
//! nothing in the emulator sleeps for one, so a case LOOKS AGAIN until the condition holds or
//! [`SETTLE_DEADLINE`] passes, and no assertion is made about elapsed time.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::DATABASE_STATUS_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    APPLICATION_STATUS_ACTIVATED, APPLICATION_STATUS_FAILED, APPLICATION_STATUS_STOPPED,
    DISCOVERY_STATUS_REGISTRATION_FAILED,
};

use crate::ec2_instance_shutdown::{registration, Emulator as WithEc2, UNHELD_INSTANCE};

/// An id inside the modelled `Ec2InstanceId` form which the owner of the account's instances
/// does not read as an instance id at all, so it resolves to no instance either — the other
/// way a discovery cannot be carried out.
const UNREADABLE_INSTANCE: &str = "i-0zzzzzzzzzzzzzzzz";

/// The `DatabaseId` the credential the shared registration builder declares derives, which is
/// the database every row below reads back.
const DATABASE_ID: &str = "HDB";

/// A second SAP HANA database name, so a registration can derive more than one database: the
/// system database of a multitenant HANA system.
const SECOND_DATABASE_NAME: &str = "SYSTEMDB";

/// How long a case is given for an outcome another clock owes.
///
/// Generous on purpose: the outcome is a stored due instant and nothing sleeps for one, so a
/// case looks again until it is reported. All this bound is for is that an outcome which never
/// comes fails instead of hanging; nothing asserts on elapsed time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long one look-again cycle pauses before reading again. Not a bound on anything.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// What a row does to its application before its database is read back.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// Stop the application, naming the connected entity to stop when the step carries one.
    Stop(Option<&'static str>),
    /// Look again until the application reports this `ApplicationStatus`.
    Until(&'static str),
}

/// One application, what is done to it, and the `DatabaseStatus` its database must report
/// afterwards.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// `None` is the instance this suite launches through EC2 itself, so the owner holds it
    /// and reports it running and the discovery of the registration succeeds; `Some` is a
    /// value that owner resolves to no instance, so it cannot.
    instance: Option<&'static str>,
    /// What is done to the application, in order, before its database is read.
    steps: &'static [Step],
    /// The `DatabaseStatus` the database has to report then.
    expected: &'static str,
}

/// The table: a never-discovered application reports nothing about its databases in any
/// status a call can leave it in, while a discovered one reports the DBMS they live in.
const ROWS: &[Row] = &[
    Row {
        case: "a registration whose discovery cannot be carried out, still under way",
        instance: Some(UNHELD_INSTANCE),
        steps: &[],
        expected: "UNKNOWN",
    },
    Row {
        case: "a registration whose discovery could not be carried out, settled",
        instance: Some(UNHELD_INSTANCE),
        steps: &[Step::Until(APPLICATION_STATUS_FAILED)],
        expected: "UNKNOWN",
    },
    Row {
        case: "an id the owner does not read as an instance id at all, settled",
        instance: Some(UNREADABLE_INSTANCE),
        steps: &[Step::Until(APPLICATION_STATUS_FAILED)],
        expected: "UNKNOWN",
    },
    Row {
        case: "a stop that overtook a registration which cannot succeed",
        instance: Some(UNHELD_INSTANCE),
        steps: &[Step::Stop(None)],
        expected: "UNKNOWN",
    },
    Row {
        case: "that same stop, settled",
        instance: Some(UNHELD_INSTANCE),
        steps: &[Step::Stop(None), Step::Until(APPLICATION_STATUS_STOPPED)],
        expected: "UNKNOWN",
    },
    Row {
        case: "a stop of the connected DBMS of an application that was never discovered",
        instance: Some(UNHELD_INSTANCE),
        steps: &[Step::Stop(Some("DBMS"))],
        expected: "UNKNOWN",
    },
    Row {
        case: "that same stop of the connected DBMS, settled",
        instance: Some(UNHELD_INSTANCE),
        steps: &[
            Step::Stop(Some("DBMS")),
            Step::Until(APPLICATION_STATUS_STOPPED),
        ],
        expected: "UNKNOWN",
    },
    Row {
        case: "a discovered application",
        instance: None,
        steps: &[Step::Until(APPLICATION_STATUS_ACTIVATED)],
        expected: "RUNNING",
    },
    Row {
        case: "a discovered application stopped without naming the DBMS",
        instance: None,
        steps: &[
            Step::Until(APPLICATION_STATUS_ACTIVATED),
            Step::Stop(None),
            Step::Until(APPLICATION_STATUS_STOPPED),
        ],
        expected: "RUNNING",
    },
    Row {
        case: "a discovered application whose connected DBMS was stopped",
        instance: None,
        steps: &[
            Step::Until(APPLICATION_STATUS_ACTIVATED),
            Step::Stop(Some("DBMS")),
            Step::Until(APPLICATION_STATUS_STOPPED),
        ],
        expected: "STOPPED",
    },
];

/// The `Database` `GetDatabase` reports for `database_id` under `application_id`.
async fn read_database(
    emulator: &WithEc2,
    case: &str,
    application_id: &str,
    database_id: &str,
) -> Value {
    let reply = emulator
        .ssm_sap(
            "/get-database",
            json!({"ApplicationId": application_id, "DatabaseId": database_id}),
        )
        .await;
    reply
        .get("Database")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Database, got {reply}"))
}

/// The `Database$Status` reported for `database_id` under `application_id`, checked against
/// the model's own catalogue of `DatabaseStatus` values.
///
/// `pub(crate)` so the suite about what a call whose precondition is a working application
/// answers (`tests/all/registration_outcome.rs`) reads a database's status through THIS one
/// helper, catalogue check included, rather than spelling a second variant of it.
pub(crate) async fn database_status(
    emulator: &WithEc2,
    case: &str,
    application_id: &str,
    database_id: &str,
) -> String {
    let database = read_database(emulator, case, application_id, database_id).await;
    let status = database
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Database Status, got {database}"))
        .to_string();
    assert!(
        DATABASE_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: Status {status:?} is not one of {DATABASE_STATUS_VALUES:?}"
    );
    status
}

/// What `GetApplication` reports for `application_id` in `(Status, DiscoveryStatus)`.
async fn read_application(emulator: &WithEc2, case: &str, application_id: &str) -> (String, String) {
    let reply = emulator
        .ssm_sap("/get-application", json!({"ApplicationId": application_id}))
        .await;
    let member = |name: &str| {
        reply
            .get("Application")
            .and_then(|application| application.get(name))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected an Application {name}, got {reply}"))
            .to_string()
    };
    (member("Status"), member("DiscoveryStatus"))
}

/// Look again until `application_id` reports `status`, which is an outcome the clock owes it.
///
/// `pub(crate)` for the same reason [`database_status`] is: a suite that has to let a change
/// the clock owes an application fall due waits for that CONDITION here rather than waiting a
/// fixed time of its own.
pub(crate) async fn wait_until(emulator: &WithEc2, case: &str, application_id: &str, status: &str) {
    let until = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (reported, discovery) = read_application(emulator, case, application_id).await;
        if reported == status {
            return;
        }
        assert!(
            Instant::now() < until,
            "case {case:?}: expected the application to reach {status}, it reports \
             {reported:?}/{discovery:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Stop `application_id`, naming `connected_entity` when the step carries one.
async fn stop(
    emulator: &WithEc2,
    case: &str,
    application_id: &str,
    connected_entity: Option<&str>,
) {
    let mut body = json!({"ApplicationId": application_id});
    if let Some(entity) = connected_entity {
        body["StopConnectedEntity"] = json!(entity);
    }
    let reply = emulator.ssm_sap("/stop-application", body).await;
    assert!(
        reply.get("OperationId").and_then(Value::as_str).is_some(),
        "case {case:?}: a stop hands out the record of its own work, got {reply}"
    );
}

/// Register `application_id` on `instance` through the shared registration builder.
async fn register(emulator: &WithEc2, case: &str, application_id: &str, instance: &str) {
    let reply = emulator
        .ssm_sap(
            "/register-application",
            registration(application_id, instance, &[]),
        )
        .await;
    assert!(
        reply.get("OperationId").and_then(Value::as_str).is_some(),
        "case {case:?}: a registration hands out the record of its own work, got {reply}"
    );
}

/// RULE: a database reports the status of the DBMS it lives in only where the service has
/// established what its application runs; where it established nothing — however the
/// application got into the status it is in, and whatever the DBMS record says — it reports
/// the model's `UNKNOWN`.
#[tokio::test]
async fn a_database_reports_its_dbms_only_where_discovery_established_the_system() {
    let emulator = WithEc2::new();
    let held = emulator.launch_running(1).await[0].clone();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let application_id = format!("dbms-known-{index}");
        let instance = row.instance.unwrap_or(held.as_str());
        register(&emulator, case, &application_id, instance).await;

        for step in row.steps {
            match step {
                Step::Stop(connected_entity) => {
                    stop(&emulator, case, &application_id, *connected_entity).await;
                }
                Step::Until(status) => wait_until(&emulator, case, &application_id, status).await,
            }
        }

        let actual = database_status(&emulator, case, &application_id, DATABASE_ID).await;
        assert_eq!(
            actual, row.expected,
            "case {case:?}: expected Database Status {:?}, got {actual:?}",
            row.expected
        );

        // A read changes nothing a later caller sees: the status is the application's state
        // rather than something this read computed.
        let again = database_status(&emulator, case, &application_id, DATABASE_ID).await;
        assert_eq!(
            again, actual,
            "case {case:?}: expected a second read to report Status {actual:?}, got {again:?}"
        );
    }
}

/// RULE, on the sequence that puts a never-discovered application into a status of its own
/// stop: the application reports that stopped status with the discovery result it never got,
/// and EVERY database it derives reports `UNKNOWN` — the two reads cannot disagree about
/// whether the service established anything about this application.
#[tokio::test]
async fn a_stopped_application_that_was_never_discovered_knows_nothing_of_its_databases() {
    let emulator = WithEc2::new();
    let case = "a stop of a registration whose discovery could not be carried out";
    let application_id = "dbms-unknown-multi";

    // Two credentials naming two SAP HANA databases, so the application derives more than one
    // database and the rule is read on each of them.
    let mut body = registration(application_id, UNHELD_INSTANCE, &[]);
    body["Credentials"] = json!([
        {"DatabaseName": DATABASE_ID, "CredentialType": "ADMIN", "SecretId": "sap/hdb/admin"},
        {
            "DatabaseName": SECOND_DATABASE_NAME,
            "CredentialType": "ADMIN",
            "SecretId": "sap/hdb/system",
        },
    ]);
    let registered = emulator.ssm_sap("/register-application", body).await;
    assert!(
        registered.get("OperationId").and_then(Value::as_str).is_some(),
        "case {case:?}: a registration hands out the record of its own work, got {registered}"
    );

    stop(&emulator, case, application_id, None).await;
    wait_until(&emulator, case, application_id, APPLICATION_STATUS_STOPPED).await;

    let (status, discovery_status) = read_application(&emulator, case, application_id).await;
    assert_eq!(
        (status.as_str(), discovery_status.as_str()),
        (
            APPLICATION_STATUS_STOPPED,
            DISCOVERY_STATUS_REGISTRATION_FAILED
        ),
        "case {case:?}: expected the stop to settle an application whose discovery never \
         succeeded, got {status:?}/{discovery_status:?}"
    );

    for database_id in [DATABASE_ID, SECOND_DATABASE_NAME] {
        let actual = database_status(&emulator, case, application_id, database_id).await;
        assert_eq!(
            actual, "UNKNOWN",
            "case {case:?}: expected database {database_id} to report Status \"UNKNOWN\", got \
             {actual:?}"
        );
    }
}
