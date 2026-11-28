// SPDX-License-Identifier: CC0-1.0

//! Adding a streaming source and an external destination, and the configuration an
//! application RUNS under.
//!
//! Every test drives the service the way a client does — one `POST /` per call through the
//! real `MultiServiceRouter`, with `X-Amz-Target: KinesisAnalytics_20180523.<Operation>`.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! * a streaming source and an external destination both belong to a SQL-based application,
//!   so `AddApplicationInput` and `AddApplicationOutput` against an application of any other
//!   runtime family are refused with the modelled `InvalidArgumentException` and store
//!   nothing;
//! * an accepted `AddApplicationInput` or `AddApplicationOutput` is taken only against the
//!   version the application is on now, stores the configuration with the id the service
//!   assigns it, and moves the application on to a new version;
//! * `DeleteApplicationOutput` drops exactly the destination its `OutputId` names, and an id
//!   the stored configuration does not carry is refused rather than answered as a delete;
//! * an application always runs under a FULL configuration: every sub-configuration that
//!   applies to its runtime is described, with `ConfigurationType` `DEFAULT` and the
//!   documented default values wherever the request supplied none — and a `DEFAULT`
//!   sub-configuration reports those defaults whatever else the request carried.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::ops::types::RUNTIME_ENVIRONMENT_VALUES;
use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::state;

use super::harness::{
    call, create, create_body, creates_applications_on, describe, error_code, error_message,
    router, router_on, runtime_mode, CUSTOMER_KEY,
};

// ---------------------------------------------------------------------------
// The streaming sources a request can carry
// ---------------------------------------------------------------------------

/// An `InputSchema` over `record_format`: the schema member is required on every `Input`,
/// and the column list is what a SQL application selects from.
fn input_schema(record_format: Value) -> Value {
    json!({
        "RecordFormat": record_format,
        "RecordColumns": [{"Name": "COL_A", "SqlType": "VARCHAR(16)", "Mapping": "$.a"}]
    })
}

/// One streaming source a caller can add: the case name, the `Input` itself, and the
/// in-app stream names the service must report for it.
///
/// The rows span the forms `Input` takes (`smithy_query(query="shape",shape="Input")`): a
/// Kinesis data stream source, a Kinesis Data Firehose delivery stream source, a source
/// with a Lambda pre-processor, both record formats, and the parallelism counts that size
/// the in-app stream list — so the rule is asserted about the shape class and not about the
/// one source the issue cited.
fn streaming_sources() -> Vec<(&'static str, Value, Vec<String>)> {
    vec![
        (
            "a kinesis data stream, JSON records, default parallelism",
            json!({
                "NamePrefix": "SOURCE_SQL_STREAM",
                "KinesisStreamsInput": {
                    "ResourceARN": "arn:aws:kinesis:us-east-1:000000000000:stream/in"
                },
                "InputSchema": input_schema(json!({
                    "RecordFormatType": "JSON",
                    "MappingParameters": {"JSONMappingParameters": {"RecordRowPath": "$"}}
                }))
            }),
            vec!["SOURCE_SQL_STREAM_001".to_string()],
        ),
        (
            "a firehose delivery stream, CSV records, parallelism 3",
            json!({
                "NamePrefix": "FIREHOSE_IN",
                "KinesisFirehoseInput": {
                    "ResourceARN": "arn:aws:firehose:us-east-1:000000000000:deliverystream/in"
                },
                "InputParallelism": {"Count": 3},
                "InputSchema": input_schema(json!({
                    "RecordFormatType": "CSV",
                    "MappingParameters": {
                        "CSVMappingParameters": {
                            "RecordRowDelimiter": "\n",
                            "RecordColumnDelimiter": ","
                        }
                    }
                }))
            }),
            (1..=3).map(|n| format!("FIREHOSE_IN_{n:03}")).collect(),
        ),
        (
            "a pre-processed source at the top of the parallelism range",
            json!({
                "NamePrefix": "PRE",
                "KinesisStreamsInput": {
                    "ResourceARN": "arn:aws:kinesis:us-east-1:000000000000:stream/pre"
                },
                "InputProcessingConfiguration": {
                    "InputLambdaProcessor": {
                        "ResourceARN": "arn:aws:lambda:us-east-1:000000000000:function:pre"
                    }
                },
                "InputParallelism": {"Count": 64},
                "InputSchema": input_schema(json!({"RecordFormatType": "JSON"}))
            }),
            (1..=64).map(|n| format!("PRE_{n:03}")).collect(),
        ),
    ]
}

/// Ask for a streaming source to be added and return `(status, body)`.
async fn add_input(
    router: &MultiServiceRouter,
    application: &str,
    version: i64,
    source: &Value,
) -> (u16, Value) {
    call(
        router,
        "AddApplicationInput",
        &json!({
            "ApplicationName": application,
            "CurrentApplicationVersionId": version,
            "Input": source,
        }),
    )
    .await
}

/// The `InputDescriptions` an application's stored configuration reports, as a JSON value
/// (`Null` when it holds none).
fn stored_inputs(detail: &Value) -> Value {
    detail["ApplicationConfigurationDescription"]["SqlApplicationConfigurationDescription"]
        ["InputDescriptions"]
        .clone()
}

// ---------------------------------------------------------------------------
// A streaming source belongs to a SQL-based application
// ---------------------------------------------------------------------------

/// `AddApplicationInput` against an application whose runtime is not a SQL one is refused
/// with HTTP 400 `InvalidArgumentException` and the sentence AWS states for that class, and
/// the refusal is inert: the application keeps its version and stores no input description.
///
/// The table is every runtime the model declares that this API still creates applications
/// on — the `Created` rows of the harness's runtime table, which is held to the generated
/// `RUNTIME_ENVIRONMENT_VALUES` (`src/ops/types/shared_types.rs`, which the model's
/// `RuntimeEnvironment` shape generates), so a runtime the issue never cited is covered —
/// crossed with the forms an `Input` takes.
#[tokio::test]
async fn adding_a_streaming_source_to_a_non_sql_application_is_refused() {
    let router = router();
    let sources = streaming_sources();

    for runtime in RUNTIME_ENVIRONMENT_VALUES
        .iter()
        .filter(|runtime| creates_applications_on(runtime))
    {
        let name = format!("no-sql-{}", runtime.replace(['-', '_'], "."));
        create(&router, &name, runtime).await;

        for (source_case, source, _) in &sources {
            let case = format!("{runtime}/{source_case}");
            let (status, refused) = add_input(&router, &name, 1, source).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a non-SQL runtime, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidArgumentException",
                "case {case:?}: expected InvalidArgumentException, got {refused}"
            );
            assert_eq!(
                error_message(&refused),
                "You cannot add sql configuration to a Flink application.",
                "case {case:?}: expected the SQL-on-Flink refusal sentence, got {refused}"
            );

            // Inert: no new version, and nothing stored under the SQL configuration.
            let described = describe(&router, &name).await;
            assert_eq!(
                described["ApplicationVersionId"].as_i64(),
                Some(1),
                "case {case:?}: a refused add must not version the application, got {described}"
            );
            assert_eq!(
                stored_inputs(&described),
                Value::Null,
                "case {case:?}: a refused add must store no input description, got {described}"
            );
        }
    }
}

/// A name no application is stored under is reported as the modelled
/// `ResourceNotFoundException`, in this service's one sentence for a lookup miss.
///
/// The names span the `ApplicationName` space (len 1..128, `^[a-zA-Z0-9_.-]+$`), with one
/// stored application present so a miss is a miss about the store and not about an empty
/// one.
#[tokio::test]
async fn adding_a_streaming_source_to_an_unstored_application_is_a_lookup_miss() {
    let router = router();
    create(&router, "stored-app", "FLINK-1_20").await;
    let longest = "n".repeat(128);
    let source = streaming_sources().remove(0).1;

    for name in ["a", "no-such-app", "probe.app_2-absent", longest.as_str()] {
        let (status, refused) = add_input(&router, name, 1, &source).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("Application '{name}' does not exist."),
            "case {name:?}: expected the lookup-miss sentence, got {refused}"
        );
    }
}

// ---------------------------------------------------------------------------
// The accepted path, and the version token it is taken against
// ---------------------------------------------------------------------------

/// Store a SQL-based application under `name` and return the router it is reachable
/// through.
///
/// `CreateApplication` no longer provisions the retired SQL runtime family (see
/// `modeled_client_errors.rs`), so the application is created through the real create path
/// and its stored `RuntimeEnvironment` is then re-stamped in the store — which is exactly
/// what an application created before that family was retired looks like, and what
/// `AddApplicationInput` reads when it decides.
async fn sql_application(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    name: &str,
    runtime: &str,
) {
    create(router, name, "FLINK-1_20").await;
    let store = state::service_state(&**engine).expect("the service holds state");
    let mut guard = store.lock();
    let application = guard
        .applications
        .get_mut(name)
        .unwrap_or_else(|| panic!("case {name:?}: the application was just created"));
    application.runtime_environment = runtime.to_string();
    application.configuration = None;
}

/// An accepted `AddApplicationInput` stores the source with the id and in-app stream names
/// the service assigns, answers them out of the store, and moves the application on to a
/// new version; a second source is refused, since a SQL-based application reads from one.
///
/// The rows are the `Input` forms of [`streaming_sources`], each on its own application,
/// and the stored source is read back through `DescribeApplication` — the round trip that
/// proves the answer came from state.
#[tokio::test]
async fn an_accepted_streaming_source_is_stored_and_versions_the_application() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    for (case, source, expected_streams) in streaming_sources() {
        let name = format!("sql-app-{}", expected_streams.len());
        sql_application(&engine, &router, &name, "SQL-1_0").await;
        let before = describe(&router, &name).await;

        let (status, added) = add_input(&router, &name, 1, &source).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the source to be added, got {status} {added}"
        );
        assert_eq!(
            added["ApplicationARN"].as_str(),
            before["ApplicationARN"].as_str(),
            "case {case:?}: expected the stored application ARN, got {added}"
        );
        assert_eq!(
            added["ApplicationVersionId"].as_i64(),
            Some(2),
            "case {case:?}: a configuration update results in a new version, got {added}"
        );
        let descriptions = added["InputDescriptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected InputDescriptions, got {added}"))
            .clone();
        assert_eq!(
            descriptions.len(),
            1,
            "case {case:?}: expected the one stored source, got {added}"
        );
        let description = &descriptions[0];
        assert_eq!(
            description["NamePrefix"].as_str(),
            source["NamePrefix"].as_str(),
            "case {case:?}: expected the supplied name prefix, got {description}"
        );
        assert!(
            description["InputId"]
                .as_str()
                .is_some_and(|id| !id.is_empty() && id.len() <= 50),
            "case {case:?}: expected a server-assigned InputId (len 1..50), got {description}"
        );
        assert_eq!(
            description["InAppStreamNames"],
            json!(expected_streams),
            "case {case:?}: expected one in-app stream per unit of parallelism, got \
             {description}"
        );
        assert_eq!(
            description["InputSchema"]["RecordFormat"]["RecordFormatType"].as_str(),
            source["InputSchema"]["RecordFormat"]["RecordFormatType"].as_str(),
            "case {case:?}: expected the supplied record format, got {description}"
        );

        // The round trip: the reader reports what the writer stored, on the new version.
        let described = describe(&router, &name).await;
        assert_eq!(
            stored_inputs(&described),
            json!(descriptions),
            "case {case:?}: expected describe to report the stored source, got {described}"
        );
        assert_eq!(
            described["ApplicationVersionId"].as_i64(),
            Some(2),
            "case {case:?}: expected the new version, got {described}"
        );
        assert_eq!(
            described["ApplicationVersionUpdatedFrom"].as_i64(),
            Some(1),
            "case {case:?}: expected the version the update moved on from, got {described}"
        );
        assert_ne!(
            described["ConditionalToken"].as_str(),
            before["ConditionalToken"].as_str(),
            "case {case:?}: the token naming the current version must be re-minted, got \
             {described}"
        );

        // One streaming source per SQL-based application: a second one is refused, on the
        // version the application is now on, and changes nothing.
        let (status, refused) = add_input(&router, &name, 2, &source).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidArgumentException"),
            "case {case:?}: expected a second source to be refused, got {status} {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!(
                "Application {name} already has a streaming source. A SQL-based application \
                 supports only one streaming source."
            ),
            "case {case:?}: expected the one-source sentence, got {refused}"
        );
        let after = describe(&router, &name).await;
        assert_eq!(
            after, described,
            "case {case:?}: a refused add must leave the application as it was, got {after}"
        );
    }
}

/// A `CurrentApplicationVersionId` that is not the version the application is on is refused
/// with the modelled `ConcurrentModificationException` and the fixed sentence for that class,
/// on this service's client-error status, and stores nothing.
///
/// The rows span the stale values a caller can send inside the modelled range
/// (`ApplicationVersionId` 1..999999999): the version after the current one, a version far
/// ahead, the published upper bound, and — after an accepted add has moved the application
/// on — the version that has just been CONSUMED.
#[tokio::test]
async fn a_stale_application_version_is_refused_as_a_concurrent_modification() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    let name = "version-token-app";
    sql_application(&engine, &router, name, "SQL-1_0").await;
    let source = streaming_sources().remove(0).1;

    for stale in [2_i64, 7, 999_999_999] {
        let (status, refused) = add_input(&router, name, stale, &source).await;
        assert_eq!(
            status, 400,
            "case {stale}: expected a stale version to be refused, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConcurrentModificationException",
            "case {stale}: expected ConcurrentModificationException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            "The provided application version ID does not match the most recent version \
             for the application.",
            "case {stale}: expected the stale-version sentence, got {refused}"
        );
        let described = describe(&router, name).await;
        assert_eq!(
            described["ApplicationVersionId"].as_i64(),
            Some(1),
            "case {stale}: a refused add must not version the application, got {described}"
        );
        assert_eq!(
            stored_inputs(&described),
            Value::Null,
            "case {stale}: a refused add must store no input description, got {described}"
        );
    }

    // The current version is accepted…
    let (status, added) = add_input(&router, name, 1, &source).await;
    assert_eq!(status, 200, "case current: AddApplicationInput got {added}");

    // …and replaying that now-consumed version is refused like any other stale one.
    let (status, refused) = add_input(&router, name, 1, &source).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ConcurrentModificationException"),
        "case consumed: expected a consumed version to be refused, got {status} {refused}"
    );
}

// ---------------------------------------------------------------------------
// The configuration an application runs under
// ---------------------------------------------------------------------------

/// The checkpointing a `DEFAULT` checkpoint configuration runs under, from the
/// `CheckpointConfiguration$ConfigurationType` doc
/// (`smithy_query(query="doc",shape="CheckpointConfiguration",member="ConfigurationType")`).
fn default_checkpoint() -> Value {
    json!({
        "ConfigurationType": "DEFAULT",
        "CheckpointingEnabled": true,
        "CheckpointInterval": 60_000,
        "MinPauseBetweenCheckpoints": 5_000
    })
}

/// The same `DEFAULT` checkpoint table for an application the service runs INTERACTIVELY
/// (a Studio notebook): the intervals are the service's, and checkpointing itself is off,
/// because what `CheckpointingEnabled` switches is a feature of a Managed Service for
/// Apache Flink application
/// (`smithy_query(query="doc",shape="CheckpointConfiguration",member="CheckpointingEnabled")`).
fn interactive_default_checkpoint() -> Value {
    let mut checkpoint = default_checkpoint();
    checkpoint["CheckpointingEnabled"] = json!(false);
    checkpoint
}

/// The CloudWatch logging a `DEFAULT` monitoring configuration runs under: the service's
/// published defaults, each a member of its modelled enum (`MetricsLevel` / `LogLevel`).
fn default_monitoring() -> Value {
    json!({
        "ConfigurationType": "DEFAULT",
        "MetricsLevel": "APPLICATION",
        "LogLevel": "INFO"
    })
}

/// The parallelism a `DEFAULT` parallelism configuration runs under: the service's
/// published defaults, with the parallelism in force equal to the configured one.
fn default_parallelism() -> Value {
    json!({
        "ConfigurationType": "DEFAULT",
        "Parallelism": 1,
        "ParallelismPerKPU": 1,
        "CurrentParallelism": 1,
        "AutoScalingEnabled": true
    })
}

/// The same `DEFAULT` parallelism table for an application the service runs INTERACTIVELY
/// (a Studio notebook): the parallelism is the service's, and the automatic scaling is off,
/// because what `AutoScalingEnabled` switches is a feature of a Managed Service for Apache
/// Flink application
/// (`smithy_query(query="doc",shape="ParallelismConfiguration",member="AutoScalingEnabled")`).
fn interactive_default_parallelism() -> Value {
    let mut parallelism = default_parallelism();
    parallelism["AutoScalingEnabled"] = json!(false);
    parallelism
}

/// Assert every member of `expected` is reported with that value in `actual`.
fn assert_members(case: &str, member: &str, actual: &Value, expected: &Value) {
    let members = expected
        .as_object()
        .expect("the expectation is a JSON object");
    for (key, value) in members {
        assert_eq!(
            &actual[key], value,
            "case {case:?}: expected {member}.{key} to be {value}, got {actual}"
        );
    }
}

/// One application to create: what configuration it is created with, and what its
/// configuration description must then report.
struct ConfigurationCase {
    case: &'static str,
    runtime: &'static str,
    mode: Option<&'static str>,
    /// The `ApplicationConfiguration` the create carries, or `Null` for none at all.
    configuration: Value,
    checkpoint: Value,
    monitoring: Value,
    parallelism: Value,
    snapshots_enabled: bool,
}

/// Every created application reports a FULL `ApplicationConfigurationDescription`: the
/// sub-configurations its runtime runs under, with `ConfigurationType` `DEFAULT` and the
/// documented default values wherever the request supplied none, the caller's values where
/// it supplied `CUSTOM` ones, and the defaults again for a `CUSTOM` member left unset.
///
/// The rows span the ways a configuration can be incomplete — no `ApplicationConfiguration`
/// at all, one with no Flink configuration, a Flink configuration carrying only one of its
/// three sub-configurations, a `DEFAULT` sub-configuration that also carries values (which
/// `DEFAULT` means the service ignores), a `CUSTOM` one with members left unset, and an
/// explicit `SnapshotsEnabled false` which is the caller's value and not a default — across
/// three runtime families.
#[tokio::test]
async fn a_created_application_reports_the_configuration_it_runs_under() {
    let cases = [
        ConfigurationCase {
            case: "no application configuration at all",
            runtime: "FLINK-1_15",
            mode: Some("STREAMING"),
            configuration: Value::Null,
            checkpoint: default_checkpoint(),
            monitoring: default_monitoring(),
            parallelism: default_parallelism(),
            snapshots_enabled: true,
        },
        ConfigurationCase {
            case: "a configuration with no flink configuration",
            runtime: "FLINK-1_20",
            mode: None,
            configuration: json!({
                "ApplicationCodeConfiguration": {
                    "CodeContentType": "PLAINTEXT",
                    "CodeContent": {"TextContent": "env.execute();"}
                }
            }),
            checkpoint: default_checkpoint(),
            monitoring: default_monitoring(),
            parallelism: default_parallelism(),
            snapshots_enabled: true,
        },
        ConfigurationCase {
            case: "a flink configuration carrying only monitoring",
            runtime: "FLINK-1_8",
            mode: None,
            configuration: json!({
                "FlinkApplicationConfiguration": {
                    "MonitoringConfiguration": {
                        "ConfigurationType": "CUSTOM",
                        "LogLevel": "DEBUG",
                        "MetricsLevel": "TASK"
                    }
                }
            }),
            checkpoint: default_checkpoint(),
            monitoring: json!({
                "ConfigurationType": "CUSTOM",
                "LogLevel": "DEBUG",
                "MetricsLevel": "TASK"
            }),
            parallelism: default_parallelism(),
            snapshots_enabled: true,
        },
        ConfigurationCase {
            case: "a DEFAULT sub-configuration that also carries values",
            runtime: "FLINK-2_3",
            mode: None,
            configuration: json!({
                "FlinkApplicationConfiguration": {
                    "CheckpointConfiguration": {
                        "ConfigurationType": "DEFAULT",
                        "CheckpointingEnabled": false,
                        "CheckpointInterval": 120_000,
                        "MinPauseBetweenCheckpoints": 90_000
                    }
                }
            }),
            checkpoint: default_checkpoint(),
            monitoring: default_monitoring(),
            parallelism: default_parallelism(),
            snapshots_enabled: true,
        },
        ConfigurationCase {
            case: "a CUSTOM sub-configuration with members left unset",
            runtime: "FLINK-1_19",
            mode: None,
            configuration: json!({
                "FlinkApplicationConfiguration": {
                    "ParallelismConfiguration": {
                        "ConfigurationType": "CUSTOM",
                        "Parallelism": 8
                    }
                }
            }),
            checkpoint: default_checkpoint(),
            monitoring: default_monitoring(),
            parallelism: json!({
                "ConfigurationType": "CUSTOM",
                "Parallelism": 8,
                "ParallelismPerKPU": 1,
                "CurrentParallelism": 8,
                "AutoScalingEnabled": true
            }),
            snapshots_enabled: true,
        },
        ConfigurationCase {
            case: "snapshots turned off by the caller",
            runtime: "ZEPPELIN-FLINK-3_0",
            mode: Some("INTERACTIVE"),
            configuration: json!({"ApplicationSnapshotConfiguration": {"SnapshotsEnabled": false}}),
            checkpoint: interactive_default_checkpoint(),
            monitoring: default_monitoring(),
            parallelism: interactive_default_parallelism(),
            snapshots_enabled: false,
        },
    ];

    let router = router();
    for ConfigurationCase {
        case,
        runtime,
        mode,
        configuration,
        checkpoint,
        monitoring,
        parallelism,
        snapshots_enabled,
    } in cases
    {
        let name = format!("defaults-{}", case.replace(' ', "-"));
        let mut body = create_body(&name, runtime);
        if let Some(mode) = mode {
            body["ApplicationMode"] = json!(mode);
        }
        if !configuration.is_null() {
            body["ApplicationConfiguration"] = configuration;
        }
        let (status, created) = call(&router, "CreateApplication", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApplication got {status} {created}"
        );
        let detail = created["ApplicationDetail"].clone();

        let described = describe(&router, &name).await;
        assert_eq!(
            described, detail,
            "case {case:?}: expected describe to repeat the created detail, got {described}"
        );

        let config = &detail["ApplicationConfigurationDescription"];
        assert!(
            config.is_object(),
            "case {case:?}: expected an ApplicationConfigurationDescription, got {detail}"
        );
        let flink = &config["FlinkApplicationConfigurationDescription"];
        assert_members(
            case,
            "CheckpointConfigurationDescription",
            &flink["CheckpointConfigurationDescription"],
            &checkpoint,
        );
        assert_members(
            case,
            "MonitoringConfigurationDescription",
            &flink["MonitoringConfigurationDescription"],
            &monitoring,
        );
        assert_members(
            case,
            "ParallelismConfigurationDescription",
            &flink["ParallelismConfigurationDescription"],
            &parallelism,
        );
        assert_eq!(
            config["ApplicationSnapshotConfigurationDescription"]["SnapshotsEnabled"].as_bool(),
            Some(snapshots_enabled),
            "case {case:?}: expected the snapshot setting the application runs under, got {config}"
        );
        // System rollback and encryption at rest are features of a Managed Service for
        // Apache Flink application, so they are described for one of those and not for a
        // Studio notebook, whose description carries only the sub-configurations it has
        // (`runtime_configuration_defaults.rs` drives that rule over the kinds).
        if mode.unwrap_or_else(|| runtime_mode(runtime)) == "INTERACTIVE" {
            assert_eq!(
                config.get("ApplicationSystemRollbackConfigurationDescription"),
                None,
                "case {case:?}: a notebook describes no system rollback it never had, \
                 got {config}"
            );
            assert_eq!(
                config.get("ApplicationEncryptionConfigurationDescription"),
                None,
                "case {case:?}: a notebook describes no encryption configuration it never \
                 had, got {config}"
            );
        } else {
            assert_eq!(
                config["ApplicationSystemRollbackConfigurationDescription"]["RollbackEnabled"]
                    .as_bool(),
                Some(false),
                "case {case:?}: the service rolls an application back only when asked, \
                 got {config}"
            );
            assert_eq!(
                config["ApplicationEncryptionConfigurationDescription"]["KeyType"].as_str(),
                Some("AWS_OWNED_KEY"),
                "case {case:?}: an application with no customer key is encrypted under the \
                 AWS-owned one, got {config}"
            );
        }
    }
}

/// A caller's own encryption configuration is reported as given, rather than replaced by
/// the AWS-owned-key default.
#[tokio::test]
async fn a_supplied_encryption_configuration_is_reported_as_given() {
    let router = router();
    let name = "customer-key-app";
    let mut body = create_body(name, "FLINK-1_20");
    body["ApplicationConfiguration"] = json!({
        "ApplicationEncryptionConfiguration": {
            "KeyType": "CUSTOMER_MANAGED_KEY",
            "KeyId": CUSTOMER_KEY
        }
    });
    let (status, created) = call(&router, "CreateApplication", &body).await;
    assert_eq!(status, 200, "CreateApplication got {status} {created}");

    let encryption = describe(&router, name).await["ApplicationConfigurationDescription"]
        ["ApplicationEncryptionConfigurationDescription"]
        .clone();
    assert_eq!(
        encryption["KeyType"].as_str(),
        Some("CUSTOMER_MANAGED_KEY"),
        "case customer-managed: expected the supplied key type, got {encryption}"
    );
    assert_eq!(
        encryption["KeyId"].as_str(),
        body["ApplicationConfiguration"]["ApplicationEncryptionConfiguration"]["KeyId"].as_str(),
        "case customer-managed: expected the supplied key id, got {encryption}"
    );
}

// ---------------------------------------------------------------------------
// The external destinations a SQL-based application writes to
// ---------------------------------------------------------------------------

/// One output configuration a caller can add: the case name and the `Output` itself.
///
/// The rows span the forms `Output` takes (`smithy_query(query="shape",shape="Output")`):
/// each of the three destinations one can identify — a Kinesis data stream, a Kinesis Data
/// Firehose delivery stream and an Amazon Lambda function — and both `RecordFormatType`
/// values a `DestinationSchema` can name, so the rule is asserted about the shape class and
/// not about the one destination a single request carried.
fn output_destinations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a kinesis data stream, JSON records",
            json!({
                "Name": "DESTINATION_SQL_STREAM",
                "KinesisStreamsOutput": {
                    "ResourceARN": "arn:aws:kinesis:us-east-1:000000000000:stream/out"
                },
                "DestinationSchema": {"RecordFormatType": "JSON"}
            }),
        ),
        (
            "a firehose delivery stream, CSV records",
            json!({
                "Name": "ERROR_SQL_STREAM",
                "KinesisFirehoseOutput": {
                    "ResourceARN": "arn:aws:firehose:us-east-1:000000000000:deliverystream/out"
                },
                "DestinationSchema": {"RecordFormatType": "CSV"}
            }),
        ),
        (
            "a lambda function, JSON records",
            json!({
                "Name": "LAMBDA_SQL_STREAM",
                "LambdaOutput": {
                    "ResourceARN": "arn:aws:lambda:us-east-1:000000000000:function:sink"
                },
                "DestinationSchema": {"RecordFormatType": "JSON"}
            }),
        ),
    ]
}

/// Ask for an external destination to be added and return `(status, body)`.
async fn add_output(
    router: &MultiServiceRouter,
    application: &str,
    version: i64,
    output: &Value,
) -> (u16, Value) {
    call(
        router,
        "AddApplicationOutput",
        &json!({
            "ApplicationName": application,
            "CurrentApplicationVersionId": version,
            "Output": output,
        }),
    )
    .await
}

/// Ask for the destination held under `output_id` to be deleted and return `(status, body)`.
async fn delete_output(
    router: &MultiServiceRouter,
    application: &str,
    version: i64,
    output_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteApplicationOutput",
        &json!({
            "ApplicationName": application,
            "CurrentApplicationVersionId": version,
            "OutputId": output_id,
        }),
    )
    .await
}

/// The `OutputDescriptions` an application's stored configuration reports, as a JSON value
/// (`Null` when it holds none).
fn stored_outputs(detail: &Value) -> Value {
    detail["ApplicationConfigurationDescription"]["SqlApplicationConfigurationDescription"]
        ["OutputDescriptions"]
        .clone()
}

/// The `OutputId` of each description in an `OutputDescriptions` list, in the order reported.
fn output_ids(descriptions: &Value) -> Vec<String> {
    descriptions
        .as_array()
        .map(|list| {
            list.iter()
                .map(|described| described["OutputId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `AddApplicationOutput` against an application whose runtime is not a SQL one is refused
/// with HTTP 400 `InvalidArgumentException` and the sentence AWS states for that class,
/// naming the configuration the request carries, and the refusal is inert: the application
/// keeps its version and stores no output description.
///
/// The table is every runtime the model declares that this API still creates applications
/// on — the harness's runtime table, held to the generated `RUNTIME_ENVIRONMENT_VALUES` —
/// crossed with the destinations an `Output` can identify, so a runtime and a destination
/// form no single request carried are covered too.
#[tokio::test]
async fn adding_an_output_to_a_non_sql_application_is_refused() {
    let router = router();
    let destinations = output_destinations();

    for runtime in RUNTIME_ENVIRONMENT_VALUES
        .iter()
        .filter(|runtime| creates_applications_on(runtime))
    {
        let name = format!("no-sql-out-{}", runtime.replace(['-', '_'], "."));
        create(&router, &name, runtime).await;

        for (destination_case, output) in &destinations {
            let case = format!("{runtime}/{destination_case}");
            let (status, refused) = add_output(&router, &name, 1, output).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a non-SQL runtime, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidArgumentException",
                "case {case:?}: expected InvalidArgumentException, got {refused}"
            );
            assert_eq!(
                error_message(&refused),
                "You cannot add sql configuration Output to a Flink application.",
                "case {case:?}: expected the SQL-on-Flink refusal sentence, got {refused}"
            );

            // Inert: no new version, and nothing stored under the SQL configuration.
            let described = describe(&router, &name).await;
            assert_eq!(
                described["ApplicationVersionId"].as_i64(),
                Some(1),
                "case {case:?}: a refused add must not version the application, got {described}"
            );
            assert_eq!(
                stored_outputs(&described),
                Value::Null,
                "case {case:?}: a refused add must store no output description, got {described}"
            );
        }
    }
}

/// A name no application is stored under is reported as the modelled
/// `ResourceNotFoundException` by both of the operations that change an application's
/// outputs, in this service's one sentence for a lookup miss.
///
/// The names span the `ApplicationName` space (len 1..128, `^[a-zA-Z0-9_.-]+$`), with one
/// stored application present so a miss is a miss about the store and not about an empty one.
#[tokio::test]
async fn an_output_change_on_an_unstored_application_is_a_lookup_miss() {
    let router = router();
    create(&router, "stored-out-app", "FLINK-1_20").await;
    let longest = "n".repeat(128);
    let output = output_destinations().remove(0).1;

    for name in ["a", "no-such-app", "probe.app_2-absent", longest.as_str()] {
        for (operation, answered) in [
            ("AddApplicationOutput", add_output(&router, name, 1, &output).await),
            ("DeleteApplicationOutput", delete_output(&router, name, 1, "1.1").await),
        ] {
            let (status, refused) = answered;
            let case = format!("{operation}/{name}");
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {refused}"
            );
            assert_eq!(
                error_message(&refused),
                format!("Application '{name}' does not exist."),
                "case {case:?}: expected the lookup-miss sentence, got {refused}"
            );
        }
    }
}

/// An accepted `AddApplicationOutput` stores the destination with the id the service assigns
/// it, answers it out of the store, and moves the application on to a new version — and a
/// further destination JOINS the ones already configured, since an application can write to
/// one or more of them.
///
/// The rows are the `Output` forms of [`output_destinations`], added in turn to the SAME
/// application, and every one is read back through `DescribeApplication` — the round trip
/// that proves the answer came from state.
#[tokio::test]
async fn an_accepted_output_is_stored_and_versions_the_application() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    let name = "sql-out-app";
    sql_application(&engine, &router, name, "SQL-1_0").await;

    let mut expected_version = 1_i64;
    let mut seen_ids: Vec<String> = Vec::new();

    for (case, output) in output_destinations() {
        let before = describe(&router, name).await;
        let (status, added) = add_output(&router, name, expected_version, &output).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the destination to be added, got {status} {added}"
        );
        expected_version += 1;
        assert_eq!(
            added["ApplicationARN"].as_str(),
            before["ApplicationARN"].as_str(),
            "case {case:?}: expected the stored application ARN, got {added}"
        );
        assert_eq!(
            added["ApplicationVersionId"].as_i64(),
            Some(expected_version),
            "case {case:?}: a configuration update results in a new version, got {added}"
        );

        let descriptions = added["OutputDescriptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected OutputDescriptions, got {added}"))
            .clone();
        assert_eq!(
            descriptions.len(),
            seen_ids.len() + 1,
            "case {case:?}: expected the destination to join the configured ones, got {added}"
        );
        let described = descriptions
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: expected the added destination, got {added}"));
        assert_eq!(
            described["Name"].as_str(),
            output["Name"].as_str(),
            "case {case:?}: expected the supplied in-application stream name, got {described}"
        );
        assert_eq!(
            described["DestinationSchema"],
            output["DestinationSchema"],
            "case {case:?}: expected the supplied destination schema, got {described}"
        );
        for (member, description_member) in [
            ("KinesisStreamsOutput", "KinesisStreamsOutputDescription"),
            ("KinesisFirehoseOutput", "KinesisFirehoseOutputDescription"),
            ("LambdaOutput", "LambdaOutputDescription"),
        ] {
            assert_eq!(
                described[description_member]["ResourceARN"].as_str(),
                output[member]["ResourceARN"].as_str(),
                "case {case:?}: expected {description_member} to describe {member}, got \
                 {described}"
            );
        }
        let id = described["OutputId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a server-assigned OutputId, got {described}"))
            .to_string();
        assert!(
            !id.is_empty() && id.len() <= 50,
            "case {case:?}: expected an OutputId the model admits (len 1..50), got {id:?}"
        );
        assert!(
            !seen_ids.contains(&id),
            "case {case:?}: expected a unique OutputId, got {id:?} again"
        );
        seen_ids.push(id);

        // The round trip: the reader reports what the writer stored, on the new version.
        let after = describe(&router, name).await;
        assert_eq!(
            stored_outputs(&after),
            json!(descriptions),
            "case {case:?}: expected describe to report the stored destinations, got {after}"
        );
        assert_eq!(
            after["ApplicationVersionId"].as_i64(),
            Some(expected_version),
            "case {case:?}: expected the new version, got {after}"
        );
        assert_ne!(
            after["ConditionalToken"].as_str(),
            before["ConditionalToken"].as_str(),
            "case {case:?}: the token naming the current version must be re-minted, got {after}"
        );
    }

    // The ceiling on how many destinations one application writes to: the table above fills
    // it exactly, so one more is refused — on the version the application is now on — and
    // leaves the application as it was.
    assert_eq!(
        seen_ids.len(),
        state::MAX_OUTPUTS_PER_APPLICATION,
        "the destination table must fill the ceiling exactly for the next case to test it, \
         got {seen_ids:?}"
    );
    let before = describe(&router, name).await;
    let (case, one_too_many) = output_destinations().remove(0);
    let (status, refused) = add_output(&router, name, expected_version, &one_too_many).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidArgumentException"),
        "case {case:?} past the ceiling: expected it to be refused, got {status} {refused}"
    );
    assert_eq!(
        error_message(&refused),
        format!(
            "Application {name} already has {max} outputs. A SQL-based application supports \
             at most {max} output destinations.",
            max = state::MAX_OUTPUTS_PER_APPLICATION
        ),
        "case {case:?} past the ceiling: expected the ceiling sentence, got {refused}"
    );
    let after = describe(&router, name).await;
    assert_eq!(
        after, before,
        "case {case:?} past the ceiling: a refused add must leave the application as it was, \
         got {after}"
    );
}

/// A `CurrentApplicationVersionId` that is not the version the application is on is refused
/// with the modelled `ConcurrentModificationException` for BOTH output changes, and stores
/// nothing.
///
/// The rows span the stale values a caller can send inside the modelled range
/// (`ApplicationVersionId` 1..999999999): the version after the current one, a version far
/// ahead, the published upper bound, and — once a destination has been added — the version
/// that has just been CONSUMED.
#[tokio::test]
async fn a_stale_application_version_is_refused_for_an_output_change() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    let name = "out-version-token-app";
    sql_application(&engine, &router, name, "SQL-1_0").await;
    let output = output_destinations().remove(0).1;

    for stale in [2_i64, 7, 999_999_999] {
        for (operation, answered) in [
            ("AddApplicationOutput", add_output(&router, name, stale, &output).await),
            ("DeleteApplicationOutput", delete_output(&router, name, stale, "1.1").await),
        ] {
            let (status, refused) = answered;
            let case = format!("{operation}/{stale}");
            assert_eq!(
                status, 400,
                "case {case:?}: expected a stale version to be refused, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ConcurrentModificationException",
                "case {case:?}: expected ConcurrentModificationException, got {refused}"
            );
            assert_eq!(
                error_message(&refused),
                "The provided application version ID does not match the most recent version \
                 for the application.",
                "case {case:?}: expected the stale-version sentence, got {refused}"
            );
        }
        let described = describe(&router, name).await;
        assert_eq!(
            described["ApplicationVersionId"].as_i64(),
            Some(1),
            "case {stale}: a refused change must not version the application, got {described}"
        );
        assert_eq!(
            stored_outputs(&described),
            Value::Null,
            "case {stale}: a refused change must store no output description, got {described}"
        );
    }

    // The current version is accepted…
    let (status, added) = add_output(&router, name, 1, &output).await;
    assert_eq!(status, 200, "case current: AddApplicationOutput got {added}");
    let stored = output_ids(&added["OutputDescriptions"]);
    let id = stored
        .first()
        .unwrap_or_else(|| panic!("case current: expected the stored OutputId, got {added}"));

    // …and replaying that now-consumed version is refused like any other stale one, for the
    // delete of a destination that IS configured as much as for another add.
    for (operation, answered) in [
        ("AddApplicationOutput", add_output(&router, name, 1, &output).await),
        ("DeleteApplicationOutput", delete_output(&router, name, 1, id).await),
    ] {
        let (status, refused) = answered;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ConcurrentModificationException"),
            "case consumed/{operation}: expected a consumed version to be refused, got \
             {status} {refused}"
        );
    }
}

/// `DeleteApplicationOutput` naming an id the application's current configuration does not
/// carry is refused with HTTP 400 `InvalidArgumentException` and the sentence AWS states for
/// that class, naming the id the request sent, and stores nothing.
///
/// The rows span the `Id` space the model admits (len 1..50, `^[a-zA-Z0-9_.-]+$`) on an
/// application that carries NO SQL configuration at all, and the same ids on a SQL
/// application that does carry a destination under another id — so the refusal is about the
/// id not being configured rather than about the application being empty.
#[tokio::test]
async fn deleting_an_output_the_application_does_not_carry_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    let flink = "no-outputs-flink-app";
    create(&router, flink, "FLINK-1_20").await;
    let sql = "no-such-output-sql-app";
    sql_application(&engine, &router, sql, "SQL-1_0").await;
    let (status, added) = add_output(&router, sql, 1, &output_destinations().remove(0).1).await;
    assert_eq!(status, 200, "AddApplicationOutput got {status} {added}");
    let configured = output_ids(&added["OutputDescriptions"]);

    let longest = "o".repeat(50);
    for (application, version, still_configured) in [
        (flink, 1_i64, Vec::new()),
        (sql, 2, configured.clone()),
    ] {
        for output_id in ["1", "1.1", "o_1-2.3", longest.as_str()] {
            assert!(
                !configured.contains(&output_id.to_string()),
                "case {output_id:?}: the ids a case deletes are not the configured one"
            );
            let case = format!("{application}/{output_id}");
            let (status, refused) = delete_output(&router, application, version, output_id).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for an unconfigured output, got {status} \
                 {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidArgumentException",
                "case {case:?}: expected InvalidArgumentException, got {refused}"
            );
            assert_eq!(
                error_message(&refused),
                format!(
                    "There are no outputs with OutputId {output_id} that you would like to \
                     delete"
                ),
                "case {case:?}: expected the no-such-output sentence, got {refused}"
            );
            let described = describe(&router, application).await;
            assert_eq!(
                described["ApplicationVersionId"].as_i64(),
                Some(version),
                "case {case:?}: a refused delete must not version the application, got \
                 {described}"
            );
            assert_eq!(
                output_ids(&stored_outputs(&described)),
                still_configured,
                "case {case:?}: a refused delete must leave the configured outputs, got \
                 {described}"
            );
        }
    }
}

/// An accepted `DeleteApplicationOutput` drops exactly the destination the id names, leaves
/// the others configured, and moves the application on to a new version; the same id is then
/// no longer there to delete.
///
/// Every destination of [`output_destinations`] is configured first, and the one deleted is
/// the MIDDLE of them, so the assertion is about the id rather than about a position.
#[tokio::test]
async fn an_accepted_delete_drops_one_output_and_versions_the_application() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    let name = "out-delete-app";
    sql_application(&engine, &router, name, "SQL-1_0").await;

    let destinations = output_destinations();
    let mut version = 1_i64;
    for (case, output) in &destinations {
        let (status, added) = add_output(&router, name, version, output).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AddApplicationOutput got {status} {added}"
        );
        version += 1;
    }
    let configured = output_ids(&stored_outputs(&describe(&router, name).await));
    assert_eq!(
        configured.len(),
        destinations.len(),
        "expected every destination to be configured, got {configured:?}"
    );
    let deleted = configured[configured.len() / 2].clone();

    let (status, answered) = delete_output(&router, name, version, &deleted).await;
    assert_eq!(
        status, 200,
        "case {deleted:?}: expected the destination to be deleted, got {status} {answered}"
    );
    version += 1;
    assert_eq!(
        answered["ApplicationVersionId"].as_i64(),
        Some(version),
        "case {deleted:?}: a configuration update results in a new version, got {answered}"
    );

    let remaining: Vec<String> = configured
        .iter()
        .filter(|id| **id != deleted)
        .cloned()
        .collect();
    let described = describe(&router, name).await;
    assert_eq!(
        output_ids(&stored_outputs(&described)),
        remaining,
        "case {deleted:?}: expected the other destinations to stay configured, got {described}"
    );
    assert_eq!(
        described["ApplicationVersionId"].as_i64(),
        Some(version),
        "case {deleted:?}: expected the new version, got {described}"
    );

    // The id is gone, so deleting it again names no configuration to delete.
    let (status, refused) = delete_output(&router, name, version, &deleted).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidArgumentException"),
        "case {deleted:?}: expected a second delete to be refused, got {status} {refused}"
    );
    let after = describe(&router, name).await;
    assert_eq!(
        after, described,
        "case {deleted:?}: a refused delete must leave the application as it was, got {after}"
    );
}
