// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue ETL registration slice: `CreateJob` / `GetJob` and
//! `CreateConnection` / `GetConnection`.
//!
//! Every call goes over the wire the way a client makes it — `POST /` with
//! `X-Amz-Target: AWSGlue.<Operation>` and an `awsJson1_1` body — through the same
//! `MultiServiceRouter::dispatch` the production front door uses, on the shared harness
//! [`super::wire`] owns.
//!
//! The rules under test, each with its source in the model's own documentation:
//!
//! * `CreateJob` creates a job DEFINITION and answers its name; `GetJob` reports that
//!   stored definition back, with the members the request supplied plus the defaults the
//!   model documents for the ones it omitted — `JobMode` ("When the JobMode field is
//!   missing or null, SCRIPT is assigned as the default value"), `GlueVersion` (the version
//!   the service fills in for a create that named none, which every read reports),
//!   `Timeout` ("the
//!   timeout is defaulted to 2,880 minutes for Glue version 4.0 and earlier, or 480
//!   minutes for Glue version 5.0 and later"),
//!   `ExecutionProperty$MaxConcurrentRuns` ("The default is 1") and the `PythonVersion`
//!   nested in the command it sent (`tests/all/job_command_defaults.rs` owns that rule).
//! * A job name is unique per account ("It must be unique in your account") and is the
//!   create's idempotency identifier, so a repeat carrying ANOTHER definition is
//!   `IdempotentParameterMismatchException`; a request contradicting a documented rule is
//!   `InvalidInputException` and writes nothing.
//! * `CreateConnection` creates a connection definition in the addressed catalog, which
//!   `GetConnection` reports back; `HidePassword` withholds the password properties from
//!   every property map the answer carries ("Allows you to retrieve the connection
//!   metadata without returning the password"),
//!   `ApplyOverrideForComputeEnvironment` overlays that environment's property map
//!   ("specifies returning properties for the specified compute environment"), and
//!   `ConnectionSchemaVersion` is 2 for a connection carrying those per-environment maps
//!   ("Version 2 supports properties for specific compute environments") and 1 otherwise.
//!
//! The suites are TABLES: each row is one input along the axis the rule generalises over
//! (the job command, the Glue version, which optional members the request carries, the
//! connection type, which property maps it sets, which compute environment is asked for),
//! and the recorded `glueetl`/`s3://…` case from the issue is one row among them.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    DEFAULT_GLUE_VERSION, DEFAULT_PYTHON_VERSION, PYTHON_VERSION_3,
};

use crate::wire::{call, emulator, error_code, ok};
/// This emulator's account id, which is also the default `CatalogId`.
const ACCOUNT_ID: &str = "000000000000";

/// A `Command` for one of the four job types `JobCommand$Name` documents.
fn command(name: &str, script: &str) -> Value {
    json!({ "Name": name, "ScriptLocation": script })
}

/// The role ARN the rows name. Glue stores `Role` as given ("The name or Amazon Resource
/// Name (ARN) of the IAM role associated with this job").
fn role() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/service-role/AWSGlueServiceRole")
}

/// One accepted-`CreateJob` row: the request body, and what the stored definition must
/// report for the four members whose default the model documents.
struct JobRow {
    case: &'static str,
    name: &'static str,
    request: Value,
    job_mode: &'static str,
    glue_version: &'static str,
    timeout: i64,
    max_concurrent_runs: i64,
}

fn job_rows() -> Vec<JobRow> {
    vec![
        JobRow {
            // The shape the issue recorded: a Spark ETL job over a script in Amazon S3.
            case: "a Spark ETL job with only the required members",
            name: "etl-minimal",
            request: json!({
                "Name": "etl-minimal",
                "Role": role(),
                "Command": command("glueetl", "s3://scripts-bucket/etl.py"),
            }),
            job_mode: "SCRIPT",
            glue_version: DEFAULT_GLUE_VERSION,
            timeout: 480,
            max_concurrent_runs: 1,
        },
        JobRow {
            case: "a Python shell job sized with MaxCapacity",
            name: "shell-job",
            request: json!({
                "Name": "shell-job",
                "Role": role(),
                "Command": {
                    "Name": "pythonshell",
                    "ScriptLocation": "s3://scripts-bucket/report.py",
                    "PythonVersion": "3",
                },
                "MaxCapacity": 0.0625,
            }),
            job_mode: "SCRIPT",
            glue_version: DEFAULT_GLUE_VERSION,
            timeout: 480,
            max_concurrent_runs: 1,
        },
        JobRow {
            case: "a streaming job on a legacy Glue version",
            name: "streaming-job",
            request: json!({
                "Name": "streaming-job",
                "Role": role(),
                "Command": command("gluestreaming", "s3://scripts-bucket/stream.scala"),
                "GlueVersion": "4.0",
                "WorkerType": "G.1X",
                "NumberOfWorkers": 2,
            }),
            job_mode: "SCRIPT",
            glue_version: "4.0",
            // Glue 4.0 is "version 4.0 and earlier", so the blank timeout is 2,880.
            timeout: 2_880,
            max_concurrent_runs: 1,
        },
        JobRow {
            case: "a Ray job with every optional member the request may carry",
            name: "ray-job",
            request: json!({
                "Name": "ray-job",
                "Role": role(),
                "Command": {
                    "Name": "glueray",
                    "ScriptLocation": "s3://scripts-bucket/ray.py",
                    "Runtime": "Ray2.4",
                },
                "Description": "a Ray job",
                "GlueVersion": "4.0",
                "JobMode": "NOTEBOOK",
                "Timeout": 60,
                "MaxRetries": 2,
                "WorkerType": "Z.2X",
                "NumberOfWorkers": 3,
                "ExecutionProperty": { "MaxConcurrentRuns": 4 },
                "DefaultArguments": { "--enable-metrics": "true" },
                "NonOverridableArguments": { "--job-language": "python" },
                "Connections": { "Connections": ["warehouse"] },
                "NotificationProperty": { "NotifyDelayAfter": 10 },
                "Tags": { "team": "data" },
                "JobRunQueuingEnabled": true,
                "MaintenanceWindow": "Sun:10",
            }),
            job_mode: "NOTEBOOK",
            glue_version: "4.0",
            timeout: 60,
            max_concurrent_runs: 4,
        },
        JobRow {
            case: "a Spark ETL job in the flexible execution class",
            name: "flex-job",
            request: json!({
                "Name": "flex-job",
                "Role": role(),
                "Command": command("glueetl", "s3://scripts-bucket/flex.py"),
                "GlueVersion": "3.0",
                "ExecutionClass": "FLEX",
                "Timeout": 10_080,
            }),
            job_mode: "SCRIPT",
            glue_version: "3.0",
            timeout: 10_080,
            max_concurrent_runs: 1,
        },
    ]
}

#[tokio::test]
async fn a_created_job_definition_reads_back_with_what_it_was_given_and_the_documented_defaults()
{
    let router = emulator();
    for row in job_rows() {
        let created = ok(&router, "CreateJob", &row.request).await;
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(row.name),
            "case {:?}: CreateJob answers the job's name, got {created}",
            row.case
        );

        let read = ok(&router, "GetJob", &json!({ "JobName": row.name })).await;
        let job = read
            .get("Job")
            .unwrap_or_else(|| panic!("case {:?}: GetJob must report a Job, got {read}", row.case));

        // Every member the request supplied round-trips unchanged.
        let sent = row
            .request
            .as_object()
            .expect("the request is a structure")
            .clone();
        for (member, value) in sent {
            // `Tags` are not part of the `Job` shape; they are read with `GetTags`.
            if member == "Tags" {
                continue;
            }
            // `ExecutionProperty` and `Timeout` are asserted below, where the default
            // matters.
            if member == "ExecutionProperty" || member == "Timeout" {
                continue;
            }
            // `Command` is asserted below too: the service fills a member in INSIDE the
            // structure the request sent (`tests/all/job_command_defaults.rs`), so what
            // round-trips is every member the request's own command carried.
            if member == "Command" {
                continue;
            }
            assert_eq!(
                job.get(&member),
                Some(&value),
                "case {:?}: member {member} must round-trip; expected {value}, got {:?}",
                row.case,
                job.get(&member)
            );
        }

        // Every member the request's `Command` carried round-trips inside the stored one,
        // which also carries the Python version the service supplies for a command that
        // named none — except a Ray job's, whose versions its `Runtime` states.
        let sent_command = row.request["Command"].clone();
        let stored_command = job
            .get("Command")
            .unwrap_or_else(|| panic!("case {:?}: a job reports its Command, got {job}", row.case));
        for (member, value) in sent_command.as_object().expect("a command is a structure") {
            assert_eq!(
                stored_command.get(member),
                Some(value),
                "case {:?}: the command's {member} must round-trip, got {stored_command}",
                row.case
            );
        }
        // A command that named a Python version keeps it; one that named none is stored
        // with the version the Glue version the stored definition RUNS ON calls for — 3
        // where that version runs no Python 2, else the historical default. A Ray job's
        // versions are its `Runtime`'s to state, so none is filled in for it.
        let runs_no_python_2 = row
            .glue_version
            .split('.')
            .next()
            .and_then(|major| major.parse::<u32>().ok())
            .is_some_and(|major| major >= 2);
        let expected_python_version = match sent_command.get("PythonVersion") {
            Some(named) => named.as_str(),
            None if sent_command["Name"] == json!("glueray") => None,
            None if runs_no_python_2 => Some(PYTHON_VERSION_3),
            None => Some(DEFAULT_PYTHON_VERSION),
        };
        assert_eq!(
            stored_command.get("PythonVersion").and_then(Value::as_str),
            expected_python_version,
            "case {:?}: expected PythonVersion {expected_python_version:?} inside the stored \
             command, got {stored_command}",
            row.case
        );

        // The four server-filled members.
        assert_eq!(
            job.get("JobMode").and_then(Value::as_str),
            Some(row.job_mode),
            "case {:?}: expected JobMode {:?}, got {:?}",
            row.case,
            row.job_mode,
            job.get("JobMode")
        );
        assert_eq!(
            job.get("GlueVersion").and_then(Value::as_str),
            Some(row.glue_version),
            "case {:?}: expected GlueVersion {:?}, got {:?}",
            row.case,
            row.glue_version,
            job.get("GlueVersion")
        );
        assert_eq!(
            job.get("Timeout").and_then(Value::as_i64),
            Some(row.timeout),
            "case {:?}: expected Timeout {}, got {:?}",
            row.case,
            row.timeout,
            job.get("Timeout")
        );
        assert_eq!(
            job.get("ExecutionProperty")
                .and_then(|property| property.get("MaxConcurrentRuns"))
                .and_then(Value::as_i64),
            Some(row.max_concurrent_runs),
            "case {:?}: expected MaxConcurrentRuns {}, got {:?}",
            row.case,
            row.max_concurrent_runs,
            job.get("ExecutionProperty")
        );
        // Both instants are server-set, equal on a definition nothing has modified, and
        // stable across reads.
        let created_on = job.get("CreatedOn").and_then(Value::as_f64);
        assert!(
            created_on.is_some(),
            "case {:?}: CreatedOn must be reported, got {job}",
            row.case
        );
        assert_eq!(
            job.get("LastModifiedOn").and_then(Value::as_f64),
            created_on,
            "case {:?}: an unmodified definition was last modified when it was created, \
             got {job}",
            row.case
        );
        let again = ok(&router, "GetJob", &json!({ "JobName": row.name })).await;
        assert_eq!(
            again.get("Job").and_then(|job| job.get("CreatedOn")),
            job.get("CreatedOn"),
            "case {:?}: CreatedOn must be stable across reads, got {again}",
            row.case
        );
    }
}

/// One refused-`CreateJob` row: the request, and the modelled error code it must answer.
struct RefusalRow {
    case: &'static str,
    request: Value,
    code: &'static str,
}

fn refusal_rows() -> Vec<RefusalRow> {
    vec![
        RefusalRow {
            case: "a timeout one minute past the documented maximum",
            request: json!({
                "Name": "long-timeout",
                "Role": role(),
                "Command": command("glueetl", "s3://scripts-bucket/etl.py"),
                "Timeout": 10_081,
            }),
            code: "InvalidInputException",
        },
        RefusalRow {
            case: "MaxCapacity beside a worker type",
            request: json!({
                "Name": "capacity-and-workers",
                "Role": role(),
                "Command": command("glueetl", "s3://scripts-bucket/etl.py"),
                "MaxCapacity": 10.0,
                "WorkerType": "G.1X",
            }),
            code: "InvalidInputException",
        },
        RefusalRow {
            case: "MaxCapacity beside a worker count",
            request: json!({
                "Name": "capacity-and-count",
                "Role": role(),
                "Command": command("glueetl", "s3://scripts-bucket/etl.py"),
                "MaxCapacity": 10.0,
                "NumberOfWorkers": 2,
            }),
            code: "InvalidInputException",
        },
        RefusalRow {
            case: "FLEX on a command that does not admit it",
            request: json!({
                "Name": "flex-shell",
                "Role": role(),
                "Command": command("pythonshell", "s3://scripts-bucket/report.py"),
                "GlueVersion": "3.0",
                "ExecutionClass": "FLEX",
            }),
            code: "InvalidInputException",
        },
        RefusalRow {
            case: "FLEX on a Glue version below 3.0",
            request: json!({
                "Name": "flex-old",
                "Role": role(),
                "Command": command("glueetl", "s3://scripts-bucket/etl.py"),
                "GlueVersion": "2.0",
                "ExecutionClass": "FLEX",
            }),
            code: "InvalidInputException",
        },
    ]
}

#[tokio::test]
async fn a_job_request_that_contradicts_a_documented_rule_is_refused_and_writes_nothing() {
    let router = emulator();
    for row in refusal_rows() {
        let (status, body) = call(&router, "CreateJob", &row.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with body {body}",
            row.case
        );
        assert!(
            error_code(&body).ends_with(row.code),
            "case {:?}: expected {}, got {}",
            row.case,
            row.code,
            error_code(&body)
        );

        // Nothing was written, so the name is still free and still unreadable.
        let name = row.request.get("Name").and_then(Value::as_str).unwrap_or("");
        let (read_status, read_body) = call(&router, "GetJob", &json!({ "JobName": name })).await;
        assert_eq!(
            read_status, 400,
            "case {:?}: a refused create must store nothing, got {read_status} {read_body}",
            row.case
        );
        assert!(
            error_code(&read_body).ends_with("EntityNotFoundException"),
            "case {:?}: expected EntityNotFoundException, got {}",
            row.case,
            error_code(&read_body)
        );
    }

    // A second create of a name already taken is refused, and the stored definition is
    // the FIRST one: the second request's role does not reach the store.
    let first = json!({
        "Name": "unique-job",
        "Role": role(),
        "Command": command("glueetl", "s3://scripts-bucket/first.py"),
    });
    ok(&router, "CreateJob", &first).await;
    let (status, body) = call(
        &router,
        "CreateJob",
        &json!({
            "Name": "unique-job",
            "Role": "arn:aws:iam::000000000000:role/other",
            "Command": command("glueetl", "s3://scripts-bucket/second.py"),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a repeated job name must be refused, got {status} {body}"
    );
    assert!(
        error_code(&body).ends_with("IdempotentParameterMismatchException"),
        "a repeated job name carrying another definition must answer \
         IdempotentParameterMismatchException, got {}",
        error_code(&body)
    );
    let read = ok(&router, "GetJob", &json!({ "JobName": "unique-job" })).await;
    assert_eq!(
        read.get("Job")
            .and_then(|job| job.get("Command"))
            .and_then(|command| command.get("ScriptLocation"))
            .and_then(Value::as_str),
        Some("s3://scripts-bucket/first.py"),
        "a refused repeat must leave the first definition in place, got {read}"
    );
}

#[tokio::test]
async fn reading_a_job_this_emulator_does_not_hold_is_the_modelled_not_found() {
    let router = emulator();
    for name in ["never-created", "etl-minimal", "a job with spaces"] {
        let (status, body) = call(&router, "GetJob", &json!({ "JobName": name })).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {name:?}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
    }
}

/// One connection row: the `ConnectionInput` sent, and the schema version the stored
/// definition must report for it.
struct ConnectionRow {
    case: &'static str,
    name: &'static str,
    input: Value,
    schema_version: i64,
}

fn connection_rows() -> Vec<ConnectionRow> {
    vec![
        ConnectionRow {
            case: "a JDBC connection with a password",
            name: "warehouse",
            input: json!({
                "Name": "warehouse",
                "ConnectionType": "JDBC",
                "ConnectionProperties": {
                    "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                    "USERNAME": "etl",
                    "PASSWORD": "a-secret",
                },
            }),
            schema_version: 1,
        },
        ConnectionRow {
            case: "a Kafka connection with match criteria",
            name: "events",
            input: json!({
                "Name": "events",
                "ConnectionType": "KAFKA",
                "ConnectionProperties": { "KAFKA_BOOTSTRAP_SERVERS": "broker.internal:9094" },
                "MatchCriteria": ["streaming", "events"],
                "Description": "the event bus",
            }),
            schema_version: 1,
        },
        ConnectionRow {
            case: "a connection with physical requirements",
            name: "mongo",
            input: json!({
                "Name": "mongo",
                "ConnectionType": "MONGODB",
                "ConnectionProperties": { "CONNECTION_URL": "mongodb://db.internal:27017" },
                "PhysicalConnectionRequirements": {
                    "SubnetId": "subnet-0123456789abcdef0",
                    "SecurityGroupIdList": ["sg-0123456789abcdef0"],
                    "AvailabilityZone": "us-east-1a",
                },
            }),
            schema_version: 1,
        },
        ConnectionRow {
            case: "a connection carrying per-compute-environment properties",
            name: "salesforce",
            input: json!({
                "Name": "salesforce",
                "ConnectionType": "SALESFORCE",
                "ConnectionProperties": { "INSTANCE_URL": "https://example.my.salesforce.com" },
                "SparkProperties": { "INSTANCE_URL": "https://spark.my.salesforce.com" },
                "PythonProperties": { "API_VERSION": "60.0" },
                "AuthenticationConfiguration": {
                    "AuthenticationType": "OAUTH2",
                    "SecretArn": "arn:aws:secretsmanager:us-east-1:000000000000:secret:sf",
                },
            }),
            schema_version: 2,
        },
    ]
}

#[tokio::test]
async fn a_created_connection_reads_back_with_what_it_was_given() {
    let router = emulator();
    for row in connection_rows() {
        let created = call(
            &router,
            "CreateConnection",
            &json!({ "ConnectionInput": row.input }),
        )
        .await;
        assert_eq!(
            created.0, 200,
            "case {:?}: CreateConnection must answer 200, got {} with body {}",
            row.case, created.0, created.1
        );

        let read = ok(&router, "GetConnection", &json!({ "Name": row.name })).await;
        let connection = read.get("Connection").unwrap_or_else(|| {
            panic!(
                "case {:?}: GetConnection must report a Connection, got {read}",
                row.case
            )
        });

        for member in [
            "Name",
            "ConnectionType",
            "Description",
            "MatchCriteria",
            "ConnectionProperties",
            "SparkProperties",
            "PythonProperties",
            "PhysicalConnectionRequirements",
        ] {
            let sent = row.input.get(member);
            if sent.is_none() {
                continue;
            }
            assert_eq!(
                connection.get(member),
                sent,
                "case {:?}: member {member} must round-trip; expected {sent:?}, got {:?}",
                row.case,
                connection.get(member)
            );
        }
        assert_eq!(
            connection
                .get("ConnectionSchemaVersion")
                .and_then(Value::as_i64),
            Some(row.schema_version),
            "case {:?}: expected ConnectionSchemaVersion {}, got {:?}",
            row.case,
            row.schema_version,
            connection.get("ConnectionSchemaVersion")
        );
        let creation_time = connection.get("CreationTime").and_then(Value::as_f64);
        assert!(
            creation_time.is_some(),
            "case {:?}: CreationTime must be reported, got {connection}",
            row.case
        );
        assert_eq!(
            connection.get("LastUpdatedTime").and_then(Value::as_f64),
            creation_time,
            "case {:?}: an unmodified connection was last updated when it was created, \
             got {connection}",
            row.case
        );
        assert!(
            connection
                .get("LastUpdatedBy")
                .and_then(Value::as_str)
                .is_some_and(|by| !by.is_empty()),
            "case {:?}: the principal that wrote the connection must be reported, got \
             {connection}",
            row.case
        );

        // A second create of the same name is refused and leaves the definition alone.
        let (status, body) = call(
            &router,
            "CreateConnection",
            &json!({ "ConnectionInput": row.input }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: a repeated connection name must be refused, got {status} {body}",
            row.case
        );
        assert!(
            error_code(&body).ends_with("AlreadyExistsException"),
            "case {:?}: expected AlreadyExistsException, got {}",
            row.case,
            error_code(&body)
        );
    }
}

#[tokio::test]
async fn a_read_withholds_the_password_and_overlays_the_compute_environment_asked_for() {
    let router = emulator();
    let input = json!({
        "Name": "warehouse",
        "ConnectionType": "JDBC",
        "ConnectionProperties": {
            "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
            "USERNAME": "etl",
            "PASSWORD": "a-secret",
            "ENCRYPTED_PASSWORD": "an-encrypted-secret",
        },
        "SparkProperties": { "USERNAME": "spark-etl", "SPARK_ONLY": "yes" },
        "AthenaProperties": { "USERNAME": "athena-etl" },
        "PythonProperties": { "PASSWORD": "a-python-secret" },
    });
    ok(
        &router,
        "CreateConnection",
        &json!({ "ConnectionInput": input }),
    )
    .await;

    // (case, HidePassword, the compute environment asked for, the properties expected)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "no hiding and no override reports every stored property",
            json!({ "Name": "warehouse" }),
            json!({
                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                "USERNAME": "etl",
                "PASSWORD": "a-secret",
                "ENCRYPTED_PASSWORD": "an-encrypted-secret",
            }),
        ),
        (
            "HidePassword withholds both password properties",
            json!({ "Name": "warehouse", "HidePassword": true }),
            json!({
                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                "USERNAME": "etl",
            }),
        ),
        (
            "the Spark override overlays that environment's properties",
            json!({ "Name": "warehouse", "ApplyOverrideForComputeEnvironment": "SPARK" }),
            json!({
                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                "USERNAME": "spark-etl",
                "SPARK_ONLY": "yes",
                "PASSWORD": "a-secret",
                "ENCRYPTED_PASSWORD": "an-encrypted-secret",
            }),
        ),
        (
            "the Athena override overlays only what it sets",
            json!({
                "Name": "warehouse",
                "HidePassword": true,
                "ApplyOverrideForComputeEnvironment": "ATHENA",
            }),
            json!({
                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                "USERNAME": "athena-etl",
            }),
        ),
        (
            "the Spark override and HidePassword together hide the overlaid password",
            json!({
                "Name": "warehouse",
                "HidePassword": true,
                "ApplyOverrideForComputeEnvironment": "SPARK",
            }),
            json!({
                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                "USERNAME": "spark-etl",
                "SPARK_ONLY": "yes",
            }),
        ),
        (
            // The overlay is applied before the password is withheld, so a password the
            // environment's own map carries is withheld too.
            "HidePassword withholds a password an environment map carries",
            json!({
                "Name": "warehouse",
                "HidePassword": true,
                "ApplyOverrideForComputeEnvironment": "PYTHON",
            }),
            json!({
                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                "USERNAME": "etl",
            }),
        ),
        (
            "without HidePassword that environment's password is reported",
            json!({
                "Name": "warehouse",
                "ApplyOverrideForComputeEnvironment": "PYTHON",
            }),
            json!({
                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                "USERNAME": "etl",
                "PASSWORD": "a-python-secret",
                "ENCRYPTED_PASSWORD": "an-encrypted-secret",
            }),
        ),
    ];

    for (case, request, expected) in rows {
        let read = ok(&router, "GetConnection", &request).await;
        let connection = read
            .get("Connection")
            .unwrap_or_else(|| panic!("case {case:?}: expected a Connection, got {read}"));
        let properties = connection
            .get("ConnectionProperties")
            .unwrap_or_else(|| panic!("case {case:?}: expected ConnectionProperties, got {read}"));
        assert_eq!(
            properties, &expected,
            "case {case:?}: expected {expected}, got {properties}"
        );
        // `HidePassword` is about the password, not about one member: no property map the
        // answer carries may disclose it.
        if request.get("HidePassword") == Some(&json!(true)) {
            for member in [
                "ConnectionProperties",
                "SparkProperties",
                "AthenaProperties",
                "PythonProperties",
            ] {
                let map = connection.get(member).cloned().unwrap_or(Value::Null);
                for key in ["PASSWORD", "ENCRYPTED_PASSWORD"] {
                    assert!(
                        map.get(key).is_none(),
                        "case {case:?}: {member} must not disclose {key} under \
                         HidePassword, got {map}"
                    );
                }
            }
        }
    }

    // An environment the connection sets NO properties for overlays nothing: the answer is
    // the stored `ConnectionProperties`, unchanged. `ComputeEnvironment` declares exactly
    // SPARK, ATHENA and PYTHON (smithy_query: shape `ComputeEnvironment`), so this needs a
    // connection that leaves one of them unset.
    let spark_only = json!({
        "Name": "spark-only",
        "ConnectionType": "JDBC",
        "ConnectionProperties": {
            "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/spark",
            "USERNAME": "etl",
        },
        "SparkProperties": { "USERNAME": "spark-etl" },
    });
    ok(
        &router,
        "CreateConnection",
        &json!({ "ConnectionInput": spark_only }),
    )
    .await;
    for (case, environment) in [
        ("an environment the connection sets no properties for", "ATHENA"),
        ("the other environment it sets none for", "PYTHON"),
    ] {
        let read = ok(
            &router,
            "GetConnection",
            &json!({
                "Name": "spark-only",
                "ApplyOverrideForComputeEnvironment": environment,
            }),
        )
        .await;
        let properties = read
            .get("Connection")
            .and_then(|connection| connection.get("ConnectionProperties"))
            .unwrap_or_else(|| panic!("case {case:?}: expected ConnectionProperties, got {read}"));
        assert_eq!(
            properties,
            &json!({
                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/spark",
                "USERNAME": "etl",
            }),
            "case {case:?}: an environment with no stored map must overlay nothing, got \
             {properties}"
        );
    }

    // Reading a connection nothing created is the modelled not-found. The CATALOG axis —
    // a read or a create addressing a catalog this account does not hold — is pinned once,
    // in `etl_connections.rs`, so the two cannot drift apart.
    let (status, body) = call(&router, "GetConnection", &json!({ "Name": "never-created" })).await;
    assert_eq!(
        status, 400,
        "reading a connection nothing created must answer HTTP 400, got {status} with body {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "reading a connection nothing created must answer EntityNotFoundException, got {}",
        error_code(&body)
    );
}
