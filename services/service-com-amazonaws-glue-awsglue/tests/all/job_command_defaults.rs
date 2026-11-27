// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the defaults a Glue job definition is STORED with, including the
//! ones nested inside the structure the caller sent.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a write of a job definition stores it with every member the SERVICE
//! supplies a documented value for filled in — a nested member of a structure the caller
//! supplied included — while a member the request declared nothing for and the service
//! documents no value for stays ABSENT. `JobCommand$PythonVersion` ("The Python version
//! being used to run a Python shell job. Allowed values are 2 or 3") is such a nested
//! member: a command that named no version is stored with the one the GLUE VERSION the
//! stored definition RUNS ON calls for — 3 where that version runs no Python 2, else the
//! historical default of 2 — written inside the structure the caller sent, and a command
//! that named a version keeps it. The version a definition runs on is the one its write
//! stated, else the one the service filled in for it.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the job
//! command, the Python version the request named (each value the member's `@pattern`
//! admits), the Glue version the write states beside it, the write that submitted the
//! definition (`CreateJob` or the `UpdateJob` that overwrites it), and the capacity form the
//! definition expresses.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    DEFAULT_GLUE_VERSION, DEFAULT_JOB_MODE, DEFAULT_MAX_CONCURRENT_RUNS, DEFAULT_PYTHON_VERSION,
    DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4, PYTHON_VERSION_3,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok};

/// The IAM role every job of these suites runs as.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/GlueJobRole";

/// The script every command of these suites points at.
const SCRIPT_LOCATION: &str = "s3://etl-scripts/etl.py";

/// The `Job` `GetJob` reports for `name`.
async fn get_job(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetJob", &json!({ "JobName": name })).await;
    body.get("Job")
        .cloned()
        .unwrap_or_else(|| panic!("GetJob must report a Job, got {body}"))
}

/// The `Command` that read reports for `name`.
async fn command_of(router: &MultiServiceRouter, name: &str) -> Value {
    let job = get_job(router, name).await;
    job.get("Command")
        .cloned()
        .unwrap_or_else(|| panic!("a read of a job must report its Command, got {job}"))
}

// ---------------------------------------------------------------------------
// The Python version a stored command carries
// ---------------------------------------------------------------------------

/// A create whose `Command` named no `PythonVersion` is stored with the service's own
/// version INSIDE that structure, whichever command the definition runs — and the members
/// the caller did send are kept beside it.
///
/// The rows differ along the job command: the two Spark commands and the Python shell one
/// (`JobCommand$Name`: "For an Apache Spark ETL job, this must be `glueetl` … for a Python
/// shell job, it must be `pythonshell` … for an Apache Spark streaming ETL job, this must be
/// `gluestreaming`"), and along the Glue version the stored definition runs on, which is
/// what decides WHICH version is filled in: the one the create states, the one the service
/// fills in for a create that states none, and the literal that leaves the choice to the
/// service.
#[tokio::test]
async fn a_command_that_named_no_python_version_is_stored_with_the_services_own() {
    let router = emulator();

    // The commands whose Python version this member states, from `JobCommand$Name`'s own
    // list; the Ray command is the row of the suite below, its versions being its
    // `Runtime`'s to state.
    for command_name in ["glueetl", "gluestreaming", "pythonshell"] {
        // (the Glue version the create states, the version that calls for). The published
        // versions either run Python 2 (0.9, 1.0) or run Python 3 alone (2.0 and later),
        // and a create that states none keeps the historical default.
        let versions: [(Option<&str>, &str); 7] = [
            // A create that states no version is stored on the service's own default, which
            // runs no Python 2, so that is the runtime the job gets.
            (None, PYTHON_VERSION_3),
            (Some("auto"), PYTHON_VERSION_3),
            (Some("0.9"), DEFAULT_PYTHON_VERSION),
            (Some("1.0"), DEFAULT_PYTHON_VERSION),
            (Some("2.0"), PYTHON_VERSION_3),
            (Some("5.0"), PYTHON_VERSION_3),
            (Some(DEFAULT_GLUE_VERSION), PYTHON_VERSION_3),
        ];
        for (index, (glue_version, expected)) in versions.into_iter().enumerate() {
            let job = format!("defaults-{command_name}-{index}");
            let mut request = json!({
                "Name": job,
                "Role": ROLE_ARN,
                "Command": { "Name": command_name, "ScriptLocation": SCRIPT_LOCATION },
            });
            if let Some(glue_version) = glue_version {
                request["GlueVersion"] = json!(glue_version);
            }
            ok(&router, "CreateJob", &request).await;

            let command = command_of(&router, &job).await;
            assert_eq!(
                command.get("PythonVersion").and_then(Value::as_str),
                Some(expected),
                "case {command_name} on Glue {glue_version:?}: a command that named no \
                 PythonVersion must be stored with the one that Glue version calls for, \
                 expected {expected:?}, got {command}"
            );
            assert_eq!(
                command.get("Name").and_then(Value::as_str),
                Some(command_name),
                "case {command_name} on Glue {glue_version:?}: the command name the create \
                 sent must round-trip, got {command}"
            );
            assert_eq!(
                command.get("ScriptLocation").and_then(Value::as_str),
                Some(SCRIPT_LOCATION),
                "case {command_name} on Glue {glue_version:?}: the script location the create \
                 sent must round-trip, got {command}"
            );
        }
    }
}

/// A create whose `Command` DID name a `PythonVersion` keeps the one it named, whichever
/// value it is.
///
/// The rows are the values `JobCommand$PythonVersion`'s `@pattern ^([2-3]|3[.]9)$` admits —
/// `2`, `3` and `3.9` — the first and the last of which no recorded create sent.
#[tokio::test]
async fn a_command_that_named_a_python_version_keeps_it() {
    let router = emulator();

    for python_version in ["2", "3", "3.9"] {
        let job = format!("declared-python-{python_version}");
        ok(
            &router,
            "CreateJob",
            &json!({
                "Name": job,
                "Role": ROLE_ARN,
                "Command": {
                    "Name": "pythonshell",
                    "ScriptLocation": SCRIPT_LOCATION,
                    "PythonVersion": python_version,
                },
            }),
        )
        .await;

        let command = command_of(&router, &job).await;
        assert_eq!(
            command.get("PythonVersion").and_then(Value::as_str),
            Some(python_version),
            "case {python_version}: a command that named a PythonVersion must keep it, got \
             {command}"
        );
    }
}

/// A Ray job's command is stored with the `Runtime` it named and no Python version filled
/// in: "In Ray jobs, `Runtime` is used to specify the versions of Ray, Python and additional
/// libraries available in your environment" (`JobCommand$Runtime`), so that structure's
/// versions are not this member's to supply.
#[tokio::test]
async fn a_ray_command_is_stored_with_its_runtime_and_no_python_version() {
    let router = emulator();
    let job = "ray-runtime";
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": job,
            "Role": ROLE_ARN,
            "Command": {
                "Name": "glueray",
                "ScriptLocation": SCRIPT_LOCATION,
                "Runtime": "Ray2.4",
            },
        }),
    )
    .await;

    let command = command_of(&router, job).await;
    assert_eq!(
        command.get("PythonVersion"),
        None,
        "a Ray command states its versions in its Runtime, so no PythonVersion is filled \
         in, got {command}"
    );
    assert_eq!(
        command.get("Runtime").and_then(Value::as_str),
        Some("Ray2.4"),
        "the runtime the create sent must round-trip, got {command}"
    );
}

/// The write that OVERWRITES a definition fills the Python version in exactly as the create
/// does — "the previous job definition is completely overwritten by this information"
/// (`UpdateJob`) — and WHICH version it fills in follows the Glue version the overwritten
/// definition is left ON: the one the update states, else the one it retains
/// (`JobUpdate$GlueVersion`).
///
/// The rows differ along the version the submitted command named (none, and each value the
/// member admits) and along the Glue version the update states beside it.
#[tokio::test]
async fn an_update_fills_the_version_the_glue_version_it_states_calls_for() {
    let router = emulator();
    let job = "updated-command";
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": job,
            "Role": ROLE_ARN,
            "Command": {
                "Name": "glueetl",
                "ScriptLocation": SCRIPT_LOCATION,
                "PythonVersion": "3.9",
            },
        }),
    )
    .await;

    // (case, what the update's command names for the version, the Glue version the update
    // states, the version the stored definition must then report).
    let rows: Vec<(&str, Option<&str>, Option<&str>, &str)> = vec![
        (
            // The definition was created without a Glue version, so it runs on the one the
            // service filled in and the update retains it.
            "an update naming neither version",
            None,
            None,
            PYTHON_VERSION_3,
        ),
        (
            "an update stating a Glue version that runs no Python 2",
            None,
            Some("4.0"),
            PYTHON_VERSION_3,
        ),
        (
            "an update stating a Glue version that runs Python 2",
            None,
            Some("1.0"),
            DEFAULT_PYTHON_VERSION,
        ),
        ("an update naming 2", Some("2"), Some("4.0"), "2"),
        ("an update naming 3", Some("3"), None, "3"),
        ("an update naming 3.9", Some("3.9"), None, "3.9"),
    ];
    for (case, named, glue_version, expected) in rows {
        let mut command = json!({ "Name": "glueetl", "ScriptLocation": SCRIPT_LOCATION });
        if let Some(named) = named {
            command["PythonVersion"] = json!(named);
        }
        let mut update = json!({ "Role": ROLE_ARN, "Command": command });
        if let Some(glue_version) = glue_version {
            update["GlueVersion"] = json!(glue_version);
        }
        ok(
            &router,
            "UpdateJob",
            &json!({ "JobName": job, "JobUpdate": update }),
        )
        .await;

        let stored = command_of(&router, job).await;
        assert_eq!(
            stored.get("PythonVersion").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: the overwritten definition must report {expected:?}, got {stored}"
        );
    }
}

/// The version the service fills in is part of the definition a REPEAT create is compared
/// against: a second create submitting the same definition is still the retry of the create
/// that stored it ("It must be unique in your account" — the name is the create's
/// idempotency identifier), so it is answered with the stored name, and one submitting a
/// different `PythonVersion` has associated that identifier with two different records,
/// which is the idempotency mismatch.
///
/// The rows differ along how the second create states the version: not at all, as the value
/// the service filled in, and as another value the member admits.
#[tokio::test]
async fn the_filled_version_is_part_of_the_definition_a_repeat_create_is_compared_against() {
    let router = emulator();

    // (case, the second create's PythonVersion, the error code it must be refused with, or
    // `None` where it is the retry and answers the stored name)
    let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("the repeat names no version either", None, None),
        (
            "the repeat states the version the service filled in",
            Some(PYTHON_VERSION_3),
            None,
        ),
        (
            "the repeat names another version",
            Some(DEFAULT_PYTHON_VERSION),
            Some("IdempotentParameterMismatchException"),
        ),
    ];

    for (index, (case, named, expected)) in rows.iter().enumerate() {
        let job = format!("repeat-create-{index}");
        let command = json!({ "Name": "glueetl", "ScriptLocation": SCRIPT_LOCATION });
        ok(
            &router,
            "CreateJob",
            &json!({ "Name": job, "Role": ROLE_ARN, "Command": command.clone() }),
        )
        .await;

        let mut repeat = command;
        if let Some(named) = named {
            repeat["PythonVersion"] = json!(named);
        }
        let (status, answer) = call(
            &router,
            "CreateJob",
            &json!({ "Name": job, "Role": ROLE_ARN, "Command": repeat }),
        )
        .await;
        match expected {
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a second create submitting another definition is refused, \
                     got {status} with {answer}"
                );
                assert!(
                    error_code(&answer).contains(expected),
                    "case {case:?}: expected {expected}, got {answer} ({})",
                    error_message(&answer)
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a second create submitting the stored definition answers \
                     200, got {status} with {answer}"
                );
                assert_eq!(
                    answer.get("Name").and_then(Value::as_str),
                    Some(job.as_str()),
                    "case {case:?}: the answer must name the stored job, got {answer}"
                );
            }
        }

        // Whichever answer it earned, the stored definition is the one the first create
        // stored: that create named no Glue version, so it was stored on the service's own,
        // which runs no Python 2.
        let stored = command_of(&router, &job).await;
        assert_eq!(
            stored.get("PythonVersion").and_then(Value::as_str),
            Some(PYTHON_VERSION_3),
            "case {case:?}: the second create changes nothing, got {stored}"
        );
    }
}

// ---------------------------------------------------------------------------
// The whole set of defaults a create applies, and the omissions it leaves
// ---------------------------------------------------------------------------

/// Every documented default a create applies is reported by the read, and every member the
/// request declared nothing for — and the service documents no value for — stays ABSENT
/// rather than becoming an empty value.
///
/// The definition sent is the smallest one that expresses a capacity as a worker
/// configuration, so the derived capacity members are under test beside the nested Python
/// version: `MaxCapacity` is the DPUs that configuration maps to ("for the `G.1X` worker
/// type, each worker maps to 1 DPU" — `CreateJob$WorkerType`) and `AllocatedCapacity` is the
/// same quantity ("this field is deprecated. Use MaxCapacity instead").
#[tokio::test]
async fn a_create_reports_every_default_it_applied_and_omits_what_it_was_not_sent() {
    let router = emulator();
    let job = "defaults-round-trip";
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": job,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": SCRIPT_LOCATION },
            "GlueVersion": "4.0",
            "NumberOfWorkers": 2,
            "WorkerType": "G.1X",
        }),
    )
    .await;

    let stored = get_job(&router, job).await;

    assert_eq!(
        stored["Command"].get("PythonVersion").and_then(Value::as_str),
        Some(PYTHON_VERSION_3),
        "the nested Python version must be filled in, and this create states a Glue version \
         that runs no Python 2, got {stored}"
    );
    assert_eq!(
        stored.get("MaxRetries").and_then(Value::as_i64),
        Some(0),
        "a definition that named no MaxRetries reports the member's own default of 0, got \
         {stored}"
    );
    assert_eq!(
        stored.get("Timeout").and_then(Value::as_i64),
        Some(i64::from(DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4)),
        "a Glue 4.0 definition that named no Timeout reports the documented default, got \
         {stored}"
    );
    assert_eq!(
        stored["ExecutionProperty"]
            .get("MaxConcurrentRuns")
            .and_then(Value::as_i64),
        Some(i64::from(DEFAULT_MAX_CONCURRENT_RUNS)),
        "a definition that named no ExecutionProperty reports the documented concurrency, \
         got {stored}"
    );
    assert_eq!(
        stored.get("JobMode").and_then(Value::as_str),
        Some(DEFAULT_JOB_MODE),
        "a definition that named no JobMode reports the documented default, got {stored}"
    );
    assert_eq!(
        stored.get("JobRunQueuingEnabled").and_then(Value::as_bool),
        Some(false),
        "a definition that named no queuing flag reports the applied one, got {stored}"
    );
    assert_eq!(
        stored.get("MaxCapacity").and_then(Value::as_f64),
        Some(2.0),
        "two G.1X workers are two DPUs, got {stored}"
    );
    assert_eq!(
        stored.get("AllocatedCapacity").and_then(Value::as_i64),
        Some(2),
        "the deprecated capacity member reports the same quantity, got {stored}"
    );
    assert_eq!(
        stored.get("GlueVersion").and_then(Value::as_str),
        Some("4.0"),
        "the Glue version the create sent must round-trip as sent, got {stored}"
    );
    for instant in ["CreatedOn", "LastModifiedOn"] {
        assert!(
            stored.get(instant).is_some(),
            "case {instant}: a read of a job must report the instant the service set, got \
             {stored}"
        );
    }

    // The members nothing was sent for, and the service documents no value for, stay out of
    // the stored definition rather than becoming empty values.
    for absent in [
        "DefaultArguments",
        "Description",
        "Connections",
        "SecurityConfiguration",
        "NonOverridableArguments",
    ] {
        assert_eq!(
            stored.get(absent),
            None,
            "case {absent}: a member the create declared nothing for must stay absent, got \
             {stored}"
        );
    }
}
