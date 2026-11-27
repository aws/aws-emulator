// SPDX-License-Identifier: CC0-1.0

//! The EFFECTIVE-CONFIGURATION projection a read reports, end to end over HTTP.
//!
//! Every assertion goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each case exercises routing, deserialization, the handler, the
//! persisted record and serialization — the same path an SDK client takes.
//!
//! The rule under test: a read of an application or of an interactive session reports
//! the configuration the service APPLIED to it — every configuration block present, each
//! carrying the value the create or update named or the documented default the service
//! filled in, under every key the service's wire response carries for it — and the endpoint a
//! session is reached at is reported as an address that leads back to this emulator.
//! Three suites, each over a SET of inputs rather than over one recorded request:
//!
//! 1. `GetApplication` reports the whole projection, for an application created with
//!    none of the blocks and for one created with each of them at a different value, and
//!    `UpdateApplication` leaves the same projection behind.
//! 2. `GetSession` always reports a session's extended-support flag, as a boolean.
//! 3. `GetSessionEndpoint` reports an endpoint a client can actually connect to: the
//!    address THIS emulator is served at, with the application and the session it names in
//!    its path — never a host AWS publishes its own endpoints at.
//! 4. A read of a JOB RUN reports the Flex settings the submit put the run under, with the
//!    documented default of the member it left out filled in, and `ListJobRuns` reports the
//!    same of it.

use serde_json::{json, Value};

use super::application_lifecycle::{
    call, create_application, emulator, get_application, get_job_run, list_job_runs, minimal_create,
    minimal_job_run, start_job_run, started_application, wait_for_transition, Answer,
};
use super::session_lifecycle::{get_session, minimal_session, session_application, start_session};
use super::update_application::update_application;

/// The base URL this emulator reports as its own while it is served on no port at all — what
/// [`::smithy_server_runtime::OperationContext::endpoint`] answers a handler in this
/// in-process harness, and a host RFC 6761 reserves so that nothing resolves it. A served
/// emulator reports the `host:port` it is reached at instead; either way the value is THIS
/// emulator's, which is what suite 3 pins.
const SERVED_BASE: &str = ::smithy_server_runtime::UNSERVED_URL_BASE;

/// The suffix of every host AWS publishes its own endpoints under: an endpoint naming one of
/// those takes a client out of the emulated environment, which is what suite 3 refuses.
const AWS_HOST_SUFFIX: &str = "amazonaws.com";

/// `AutoStopConfig$idleTimeoutMinutes` is `@range(min: 1, max: 10080)` and documents
/// "Defaults to 15 minutes" (`smithy_query(query="shape",shape="AutoStopConfig")`).
const DEFAULT_IDLE_TIMEOUT_MINUTES: i64 = 15;
const MIN_IDLE_TIMEOUT_MINUTES: i64 = 1;
const MAX_IDLE_TIMEOUT_MINUTES: i64 = 10080;

/// `FlexExecution$enabled` "enables Amazon EMR Serverless Flex capacity for the job run and
/// defaults to false" (`smithy_query(query="shape",shape="FlexExecution")`), written out here
/// rather than read from `crate::state` so a wrong default in `src/` cannot make this pass.
const DEFAULT_FLEX_EXECUTION_ENABLED: bool = false;

/// A KMS key ARN satisfying `EncryptionKeyArn`'s pattern
/// (`smithy_query(query="shape",shape="ManagedPersistenceMonitoringConfiguration")`).
const KEY_ARN: &str = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-0000-0000-000000000001";

/// The extended-support projection every application reports while nothing has changed
/// the account's setting: off, set at the account, never changed.
fn expected_extended_support() -> Value {
    json!({
        "enabled": false,
        "source": "ACCOUNT",
        "updatedAt": null,
        "updatedBy": null,
    })
}

/// One row of the application table: what the create asks for, and the effective
/// configuration a read must then report.
struct ApplicationCase {
    case: &'static str,
    /// The configuration members the create names, beside the required ones.
    configuration: Value,
    /// `autoStartConfiguration.enabled`.
    auto_start: bool,
    /// `autoStopConfiguration.enabled`.
    auto_stop: bool,
    /// `autoStopConfiguration.idleTimeoutMinutes`, and the same value under the other
    /// name the block carries it under.
    idle_timeout: i64,
    /// `monitoringConfiguration.managedPersistenceMonitoringConfiguration.enabled`.
    managed_persistence: bool,
    /// `monitoringConfiguration.managedPersistenceMonitoringConfiguration.encryptionKeyArn`,
    /// which has no default: absent unless the create named one.
    managed_persistence_key: Option<&'static str>,
    /// `jobLevelCostAllocationConfiguration.enabled`.
    cost_allocation: bool,
}

/// The rows: an application created with none of these blocks, and one per block at a
/// value that is not its default — including both published bounds of the idle timeout,
/// each `enabled` flag inverted, and the key the managed-persistence block admits.
fn application_cases() -> Vec<ApplicationCase> {
    vec![
        ApplicationCase {
            case: "none of the configuration blocks",
            configuration: json!({}),
            auto_start: true,
            auto_stop: true,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "auto-start and auto-stop both off",
            configuration: json!({
                "autoStartConfiguration": { "enabled": false },
                "autoStopConfiguration": { "enabled": false },
            }),
            auto_start: false,
            auto_stop: false,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "the idle timeout at its published minimum",
            configuration: json!({
                "autoStopConfiguration": { "idleTimeoutMinutes": MIN_IDLE_TIMEOUT_MINUTES },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: MIN_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "the idle timeout at its published maximum",
            configuration: json!({
                "autoStopConfiguration": { "idleTimeoutMinutes": MAX_IDLE_TIMEOUT_MINUTES },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: MAX_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "a monitoring configuration naming another of its blocks",
            configuration: json!({
                "monitoringConfiguration": {
                    "s3MonitoringConfiguration": { "logUri": "s3://bucket/logs" },
                },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "managed log persistence turned off",
            configuration: json!({
                "monitoringConfiguration": {
                    "managedPersistenceMonitoringConfiguration": { "enabled": false },
                },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: false,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "managed log persistence with an encryption key",
            configuration: json!({
                "monitoringConfiguration": {
                    "managedPersistenceMonitoringConfiguration": {
                        "encryptionKeyArn": KEY_ARN,
                    },
                },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: Some(KEY_ARN),
            cost_allocation: false,
        },
        ApplicationCase {
            case: "job level cost allocation turned on",
            configuration: json!({
                "jobLevelCostAllocationConfiguration": { "enabled": true },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: true,
        },
        ApplicationCase {
            case: "every block at once, each away from its default",
            configuration: json!({
                "autoStartConfiguration": { "enabled": false },
                "autoStopConfiguration": { "enabled": true, "idleTimeoutMinutes": 120 },
                "monitoringConfiguration": {
                    "managedPersistenceMonitoringConfiguration": {
                        "enabled": false,
                        "encryptionKeyArn": KEY_ARN,
                    },
                },
                "jobLevelCostAllocationConfiguration": { "enabled": true },
            }),
            auto_start: false,
            auto_stop: true,
            idle_timeout: 120,
            managed_persistence: false,
            managed_persistence_key: Some(KEY_ARN),
            cost_allocation: true,
        },
    ]
}

/// The `CreateApplication` body for one row.
fn create_for(case: &ApplicationCase, token: &str) -> Value {
    let mut body = minimal_create(token);
    for (member, value) in case.configuration.as_object().into_iter().flatten() {
        body[member.as_str()] = value.clone();
    }
    body
}

/// Assert the whole effective-configuration projection one row expects, over whichever
/// answer carries the application (`GetApplication`'s or `UpdateApplication`'s).
fn assert_projection(label: &str, source: &str, answer: &Answer, case: &ApplicationCase) {
    let expected_auto_stop = json!({
        "enabled": case.auto_stop,
        "idleTimeoutMinutes": case.idle_timeout,
        "idleTimeout": case.idle_timeout,
    });
    let expected_auto_start = json!({ "enabled": case.auto_start });
    let mut expected_managed = json!({ "enabled": case.managed_persistence });
    if let Some(key) = case.managed_persistence_key {
        expected_managed["encryptionKeyArn"] = json!(key);
    }

    for (pointer, expected) in [
        // Both spellings of the auto-start block, carrying the same setting.
        ("/application/autoStartConfiguration", expected_auto_start.clone()),
        ("/application/autoStartConfig", expected_auto_start),
        // Both spellings of the auto-stop block, each carrying the idle timeout under
        // both of ITS names.
        ("/application/autoStopConfiguration", expected_auto_stop.clone()),
        ("/application/autoStopConfig", expected_auto_stop),
        (
            "/application/monitoringConfiguration/managedPersistenceMonitoringConfiguration",
            expected_managed,
        ),
        (
            "/application/jobLevelCostAllocationConfiguration",
            json!({ "enabled": case.cost_allocation }),
        ),
        (
            "/application/extendedSupportConfiguration",
            expected_extended_support(),
        ),
    ] {
        assert_eq!(
            answer.at(pointer),
            expected,
            "case {label:?}: {source}{pointer} expected {expected:?}, got {:?}",
            answer.at(pointer)
        );
    }
}

/// Suite 1: every read of an application reports the configuration the service applied,
/// under every key its response carries — and an update leaves the same projection.
#[tokio::test]
async fn a_read_reports_the_applications_effective_configuration() {
    for (index, case) in application_cases().into_iter().enumerate() {
        let router = emulator();
        let token = format!("tok-projection-{index}");
        let created = create_application(&router, create_for(&case, &token)).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateApplication expected 200, got {} with {:?}",
            case.case, created.status, created.body
        );
        let id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();

        let fetched = get_application(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: GetApplication expected 200, got {} with {:?}",
            case.case, fetched.status, fetched.body
        );
        assert_projection(case.case, "GetApplication", &fetched, &case);

        // The same projection survives an update that names nothing of it: the record is
        // what carries the configuration, so the reader reports it again unchanged.
        wait_for_transition();
        let updated = update_application(
            &router,
            &id,
            json!({ "clientToken": format!("upd-{index}"), "releaseLabel": "emr-7.13.0" }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: UpdateApplication expected 200, got {} with {:?}",
            case.case, updated.status, updated.body
        );
        assert_projection(case.case, "UpdateApplication", &updated, &case);
        let read_back = get_application(&router, &id).await;
        assert_projection(case.case, "GetApplication after the update", &read_back, &case);
    }
}

/// Suite 1b: an update that REPLACES a configuration block gets the same defaults
/// materialised into it, under every key, as a create does.
#[tokio::test]
async fn an_update_reports_the_blocks_it_replaces_in_full() {
    // (case, the block the update names, the idle timeout that then applies, whether
    // managed log persistence is then on)
    let cases: Vec<(&str, Value, i64, bool)> = vec![
        (
            "an auto-stop block naming only the timeout",
            json!({ "autoStopConfiguration": { "idleTimeoutMinutes": MIN_IDLE_TIMEOUT_MINUTES } }),
            MIN_IDLE_TIMEOUT_MINUTES,
            true,
        ),
        (
            "an auto-stop block naming only the flag",
            json!({ "autoStopConfiguration": { "enabled": true } }),
            DEFAULT_IDLE_TIMEOUT_MINUTES,
            true,
        ),
        (
            "an empty monitoring configuration",
            json!({ "monitoringConfiguration": {} }),
            DEFAULT_IDLE_TIMEOUT_MINUTES,
            true,
        ),
        (
            "a monitoring configuration turning managed log persistence off",
            json!({
                "monitoringConfiguration": {
                    "managedPersistenceMonitoringConfiguration": { "enabled": false },
                },
            }),
            DEFAULT_IDLE_TIMEOUT_MINUTES,
            false,
        ),
    ];

    for (index, (case, block, idle_timeout, managed_persistence)) in
        cases.into_iter().enumerate()
    {
        let router = emulator();
        let created =
            create_application(&router, minimal_create(&format!("tok-replace-{index}"))).await;
        let id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        wait_for_transition();

        let mut body = json!({ "clientToken": format!("repl-{index}") });
        for (member, value) in block.as_object().into_iter().flatten() {
            body[member.as_str()] = value.clone();
        }
        let updated = update_application(&router, &id, body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateApplication expected 200, got {} with {:?}",
            updated.status, updated.body
        );

        for (pointer, expected) in [
            (
                "/application/autoStopConfiguration/idleTimeoutMinutes",
                json!(idle_timeout),
            ),
            (
                "/application/autoStopConfiguration/idleTimeout",
                json!(idle_timeout),
            ),
            ("/application/autoStopConfig/idleTimeout", json!(idle_timeout)),
            (
                "/application/monitoringConfiguration/managedPersistenceMonitoringConfiguration/enabled",
                json!(managed_persistence),
            ),
            (
                "/application/jobLevelCostAllocationConfiguration/enabled",
                json!(false),
            ),
        ] {
            assert_eq!(
                updated.at(pointer),
                expected,
                "case {case:?}: UpdateApplication{pointer} expected {expected:?}, got {:?}",
                updated.at(pointer)
            );
        }

        // ROUND TRIP: the reader reports exactly what the update answered.
        let read_back = get_application(&router, &id).await;
        assert_eq!(
            read_back.at("/application"),
            updated.at("/application"),
            "case {case:?}: GetApplication must report the updated record, got {:?} against \
             {:?}",
            read_back.at("/application"),
            updated.at("/application")
        );
    }
}

/// Suite 2: `GetSession` reports the session's extended-support flag, as a boolean,
/// whatever the start asked for and whenever the read arrives.
#[tokio::test]
async fn a_read_reports_a_sessions_extended_support_flag() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-session-projection").await;

    // The rows differ along what the start named and WHEN the read arrives: inside the
    // span the clock still owes the session its started state, and after it.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("only the required members", minimal_session("tok-flag-0"), false),
        (
            "a named session, read after it comes up",
            {
                let mut body = minimal_session("tok-flag-1");
                body["name"] = json!("projection");
                body
            },
            true,
        ),
        (
            "a session carrying its own idle timeout",
            {
                let mut body = minimal_session("tok-flag-2");
                body["idleTimeoutMinutes"] = json!(30);
                body
            },
            false,
        ),
        (
            "a session carrying tags, read after it comes up",
            {
                let mut body = minimal_session("tok-flag-3");
                body["tags"] = json!({ "team": "data" });
                body
            },
            true,
        ),
    ];

    for (case, body, settle) in cases {
        let started = start_session(&router, &application_id, body).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartSession expected 200, got {} with {:?}",
            started.status, started.body
        );
        let session_id = started
            .at("/sessionId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if settle {
            wait_for_transition();
        }

        let fetched = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetSession expected 200, got {} with {:?}",
            fetched.status, fetched.body
        );
        let flag = fetched.at("/session/extendedSupportEnabled");
        assert_eq!(
            flag,
            json!(false),
            "case {case:?}: GetSession must report the extended-support flag as a boolean, \
             got {flag:?}"
        );
    }
}

/// Suite 3: the endpoint reported for a session is an address of THIS emulator — the base a
/// later request addresses it at, with the application and the session in its path — and two
/// sessions never share one.
#[tokio::test]
async fn a_session_endpoint_leads_back_to_this_emulator() {
    let router = emulator();
    let (shared_application, _) = session_application(&router, "tok-origin-app").await;

    // The rows differ along which application the session runs on and whether the read
    // arrives before or after the clock owed the session its started state — neither of
    // which the host form depends on.
    let cases: Vec<(&str, bool, bool)> = vec![
        ("on a shared application, read straight away", false, false),
        ("on a shared application, read once started", false, true),
        ("on an application of its own", true, false),
        ("on an application of its own, read once started", true, true),
    ];

    let mut seen: Vec<(String, String)> = Vec::new();
    for (index, (case, own_application, settle)) in cases.into_iter().enumerate() {
        let application_id = if own_application {
            session_application(&router, &format!("tok-origin-app-{index}"))
                .await
                .0
        } else {
            shared_application.clone()
        };
        let started = start_session(
            &router,
            &application_id,
            minimal_session(&format!("tok-origin-{index}")),
        )
        .await;
        let session_id = started
            .at("/sessionId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if settle {
            wait_for_transition();
        }

        let answer = call(
            &router,
            "GET",
            &format!("/applications/{application_id}/sessions/{session_id}/endpoint"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: GetSessionEndpoint expected 200, got {} with {:?}",
            answer.status, answer.body
        );

        let endpoint = answer
            .at("/endpoint")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected =
            format!("{SERVED_BASE}/applications/{application_id}/sessions/{session_id}");
        assert_eq!(
            endpoint, expected,
            "case {case:?}: endpoint expected {expected:?}, got {endpoint:?}"
        );
        // A client takes this value and CONNECTS to it, so its authority is this emulator's
        // own and never one of AWS's hosts.
        let authority = endpoint
            .strip_prefix("https://")
            .unwrap_or(&endpoint)
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string();
        assert!(
            !authority.ends_with(AWS_HOST_SUFFIX),
            "case {case:?}: the endpoint must not name a host AWS publishes its own \
             endpoints at, got {endpoint:?}"
        );
        assert!(
            endpoint.contains(&format!("/{application_id}/")) && endpoint.ends_with(&session_id),
            "case {case:?}: the endpoint's path names the application and the session, got \
             {endpoint:?}"
        );

        seen.push((session_id, endpoint));
    }

    for (index, (session_id, endpoint)) in seen.iter().enumerate() {
        for (other_id, other_endpoint) in seen.iter().skip(index + 1) {
            assert_ne!(
                endpoint, other_endpoint,
                "sessions {session_id:?} and {other_id:?} must not share an endpoint, got \
                 {endpoint:?} twice"
            );
        }
    }
}

/// Suite 4: a read of a JOB RUN reports the Flex settings the submit put it under — the block
/// the submit carried, with `enabled` resolved to the value in force, and nothing at all for a
/// submit that carried none, which is what "if you don't specify these settings, the job run
/// doesn't use Flex capacity" leaves to report. `GetJobRun` and the `ListJobRuns` summary
/// answer the same, because both read the one stored record.
#[tokio::test]
async fn a_read_reports_the_flex_settings_a_job_run_was_submitted_under() {
    let router = emulator();
    let application_id = started_application(&router, "tok-flex-app").await;

    // The rows differ along what the submit carried: no block at all, the block with no
    // member, and the member at each of its two values.
    let cases: Vec<(&str, Option<Value>, Value)> = vec![
        ("no Flex settings at all", None, Value::Null),
        (
            "the block with no member, so its documented default is in force",
            Some(json!({})),
            json!({ "enabled": DEFAULT_FLEX_EXECUTION_ENABLED }),
        ),
        (
            "Flex capacity asked for",
            Some(json!({ "enabled": true })),
            json!({ "enabled": true }),
        ),
        (
            "Flex capacity declined",
            Some(json!({ "enabled": false })),
            json!({ "enabled": false }),
        ),
    ];

    for (index, (case, submitted, expected)) in cases.into_iter().enumerate() {
        let mut body = minimal_job_run(&format!("tok-flex-{index}"));
        if let Some(flex_execution) = submitted.clone() {
            body["flexExecution"] = flex_execution;
        }
        let started = start_job_run(&router, &application_id, body).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartJobRun expected 200, got {} with {:?}",
            started.status, started.body
        );
        let job_run_id = started
            .at("/jobRunId")
            .as_str()
            .unwrap_or_default()
            .to_string();

        let fetched = get_job_run(&router, &application_id, &job_run_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetJobRun expected 200, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/jobRun/flexExecution"),
            expected,
            "case {case:?}: a submit carrying {submitted:?} reads back as {expected:?}, got {:?}",
            fetched.at("/jobRun/flexExecution")
        );

        // The summary a list serves says of the run what a read of it says.
        let listed = list_job_runs(&router, &application_id, "").await;
        let summary = listed
            .body
            .get("jobRuns")
            .and_then(Value::as_array)
            .and_then(|entries| entries.iter().find(|entry| entry["id"] == json!(job_run_id)))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            summary.get("flexExecution").cloned().unwrap_or(Value::Null),
            expected,
            "case {case:?}: the ListJobRuns summary must report the same Flex settings as \
             GetJobRun ({expected:?}), got {summary:?}"
        );
    }
}
