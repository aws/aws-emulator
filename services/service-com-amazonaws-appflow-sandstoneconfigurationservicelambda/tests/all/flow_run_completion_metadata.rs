// SPDX-License-Identifier: CC0-1.0

//! What `DescribeFlowExecutionRecords` reports about a run that has FINISHED: the outcome
//! it reached, in words and in numbers.
//!
//! The rules under test:
//!
//! * a run that has reached a terminal status accounts for what it did in full — the
//!   sentence it ended with (`executionMessage`), its own accounting block
//!   (`executionMetadata`), and every counter `executionResult` declares, zero included
//!   (`maxPageSize`, `numParallelProcesses`) — all read off the one stored run, so the two
//!   blocks of a record can never disagree; a run that has not finished has no accounting
//!   to report and reports none;
//! * the two instants a run carries are reported under BOTH the names AppFlow reports them
//!   under: `startedAt`/`lastUpdatedAt` and `executionStartTime`/`lastUpdatedTime`;
//! * `dataPullStartTime`/`dataPullEndTime` bound the window of new or updated records a run
//!   queried its source for, so only a run of a flow that DECLARES an incremental data pull
//!   reports them — whatever volume that run moved;
//! * the message of the most recent run is the one sentence the flow reports for it too
//!   (`lastRunExecutionDetails.mostRecentExecutionMessage`).
//!
//! The wire half is driven end to end over `restJson1`, on the ONE shared engine AppFlow
//! and Amazon S3 both live on, so the counters asserted here are the ones a real transfer
//! between two locations in this emulator came to. The projection's own table (every
//! terminal status, and a run that has not reached one) is driven through the stored run,
//! which is the only way to observe a status no transfer here produces.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::ops::types::{
    ErrorInfo, ExecutionResult, IncrementalPullConfig, ScheduledTriggerProperties,
    SourceFlowConfig, TriggerConfig, TriggerProperties,
};
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::state::{
    ExecutionStatus, FlowExecution, EXECUTION_NOT_PERFORMED_MESSAGE, RUN_SUCCEEDED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::flow_lifecycle::{
    appflow_with_engine, call, on_demand_trigger, s3_destination, s3_source, salesforce_source,
    settled_runs, stored_flow, FIXTURE_ACCESS_KEY_ID, REGION,
};

/// AppFlow and Amazon S3 on one engine: the AppFlow calls reach the control plane, the
/// Amazon S3 call seeds the source location the run reads.
fn appflow_and_object_store() -> MultiServiceRouter {
    let (mut router, engine) = appflow_with_engine();
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// `PUT /<bucket>/<key>` — Amazon S3's `PutObject`, signed (in the routing sense) for the
/// object store rather than for AppFlow.
async fn put_object(router: &MultiServiceRouter, bucket: &str, key: &str, body: &str) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("https://s3.us-east-1.amazonaws.com/{bucket}/{key}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={FIXTURE_ACCESS_KEY_ID}/20260101/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(body.as_bytes().to_vec()))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "expected PutObject of {bucket}/{key} to succeed, got {}",
        response.status()
    );
}

/// A flow's Amazon S3 source, carrying an `incrementalPullConfig` naming
/// `datetime_field` when the flow declares one.
fn s3_source_pulling_from(bucket: &str, datetime_field: Option<&str>) -> Value {
    let mut source = s3_source(bucket);
    if let Some(field) = datetime_field {
        source["incrementalPullConfig"] = json!({ "datetimeTypeFieldName": field });
    }
    source
}

/// One row of the finished-run table.
struct Finished {
    case: &'static str,
    /// The fixture buckets this row's flow reads and writes.
    source_bucket: &'static str,
    destination_bucket: &'static str,
    /// The objects the source prefix holds, as (key, bytes).
    objects: &'static [(&'static str, &'static str)],
    /// The datetime field the flow's `incrementalPullConfig` names, when it declares one.
    datetime_field: Option<&'static str>,
    /// `true` for a flow whose source is a connector application this emulator does not
    /// reach, so no transfer is performed at all.
    unreachable_source: bool,
    expected_status: ExecutionStatus,
    /// The sentence the run is expected to end with.
    expected_message: &'static str,
    /// The records the run is expected to report having processed.
    expected_records: i64,
}

/// The table. The rows differ along everything the rules generalise over: how many records
/// and bytes the run moved (two, one, none), whether the flow declares an incremental data
/// pull, and whether the run succeeded or ended in the service's failure state.
fn finished_runs() -> Vec<Finished> {
    vec![
        Finished {
            case: "two records moved, no incremental pull declared",
            source_bucket: "orders-in",
            destination_bucket: "orders-out",
            objects: &[("inbound/orders.json", r#"[{"id":"1"},{"id":"2"}]"#)],
            datetime_field: None,
            unreachable_source: false,
            expected_status: ExecutionStatus::Successful,
            expected_message: RUN_SUCCEEDED_MESSAGE,
            expected_records: 2,
        },
        Finished {
            case: "one record moved, an incremental pull over a datetime field",
            source_bucket: "maps-in",
            destination_bucket: "maps-out",
            objects: &[(
                "inbound/one.json",
                r#"{"id":"7","updated_at":"2026-01-01T00:00:00Z"}"#,
            )],
            datetime_field: Some("updated_at"),
            unreachable_source: false,
            expected_status: ExecutionStatus::Successful,
            expected_message: RUN_SUCCEEDED_MESSAGE,
            expected_records: 1,
        },
        Finished {
            case: "a source prefix holding no file, no incremental pull declared",
            source_bucket: "sparse-in",
            destination_bucket: "sparse-out",
            objects: &[],
            datetime_field: None,
            unreachable_source: false,
            expected_status: ExecutionStatus::Successful,
            expected_message: RUN_SUCCEEDED_MESSAGE,
            expected_records: 0,
        },
        Finished {
            case: "a source prefix holding no file, with an incremental pull declared",
            source_bucket: "populated-in",
            destination_bucket: "populated-out",
            objects: &[],
            datetime_field: Some("LastModifiedDate"),
            unreachable_source: false,
            expected_status: ExecutionStatus::Successful,
            expected_message: RUN_SUCCEEDED_MESSAGE,
            expected_records: 0,
        },
        Finished {
            case: "a source this emulator does not reach, so nothing was transferred",
            source_bucket: "runs-in",
            destination_bucket: "runs-out",
            objects: &[],
            datetime_field: None,
            unreachable_source: true,
            expected_status: ExecutionStatus::Error,
            expected_message: EXECUTION_NOT_PERFORMED_MESSAGE,
            expected_records: 0,
        },
    ]
}

/// A number a response member carries, or `None` when the member is absent or null.
fn number(value: &Value, member: &str) -> Option<i64> {
    value.get(member).and_then(Value::as_i64)
}

/// An instant a response member carries, as the epoch seconds `restJson1` renders a
/// timestamp as, or `None` when the member is absent or null.
fn instant(value: &Value, member: &str) -> Option<f64> {
    value.get(member).and_then(Value::as_f64)
}

/// Every member a finished run accounts for is reported, from the run's own transfer.
#[tokio::test]
async fn a_finished_run_reports_the_outcome_and_the_accounting_of_what_it_did() {
    for row in finished_runs() {
        let case = row.case;
        let router = appflow_and_object_store();
        for (key, body) in row.objects {
            put_object(&router, row.source_bucket, key, body).await;
        }

        let flow_name = "completion-metadata-flow";
        let source = if row.unreachable_source {
            salesforce_source("Account")
        } else {
            s3_source_pulling_from(row.source_bucket, row.datetime_field)
        };
        let created = call(
            &router,
            "/create-flow",
            &json!({
                "flowName": flow_name,
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": source,
                "destinationFlowConfigList": [s3_destination(row.destination_bucket)],
                "tasks": [map_all()],
            }),
        )
        .await;
        assert!(
            created.is_ok(),
            "case {case:?}: expected CreateFlow to succeed, got {} {:?}",
            created.status,
            created.body
        );

        let started = call(&router, "/start-flow", &json!({ "flowName": flow_name })).await;
        assert!(
            started.is_ok(),
            "case {case:?}: expected StartFlow to succeed, got {} {:?}",
            started.status,
            started.body
        );

        let runs = settled_runs(&router, flow_name).await;
        let run = runs.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            run.get("executionStatus").and_then(Value::as_str),
            Some(row.expected_status.as_str()),
            "case {case:?}: expected the run to end {}, got {run:?}",
            row.expected_status.as_str()
        );

        // The sentence the run ended with, and the same sentence on the flow's own
        // summary of its most recent run.
        assert_eq!(
            run.get("executionMessage").and_then(Value::as_str),
            Some(row.expected_message),
            "case {case:?}: expected the run to report how it went, got {run:?}"
        );
        let described = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
        assert_eq!(
            described
                .get("lastRunExecutionDetails")
                .get("mostRecentExecutionMessage")
                .and_then(Value::as_str),
            Some(row.expected_message),
            "case {case:?}: expected the flow to carry its most recent run's message, got {:?}",
            described.get("lastRunExecutionDetails")
        );

        // Every counter `executionResult` declares, zero included.
        let result = run.get("executionResult").cloned().unwrap_or(Value::Null);
        assert_eq!(
            number(&result, "recordsProcessed"),
            Some(row.expected_records),
            "case {case:?}: expected {} records processed, got {result:?}",
            row.expected_records
        );
        assert_eq!(
            number(&result, "maxPageSize"),
            Some(0),
            "case {case:?}: expected maxPageSize to be reported as the zero it is for a \
             connector with no page size, got {result:?}"
        );
        let expected_processes = if row.unreachable_source { 0 } else { 1 };
        assert_eq!(
            number(&result, "numParallelProcesses"),
            Some(expected_processes),
            "case {case:?}: expected numParallelProcesses to report the processes that ran \
             the transfer, got {result:?}"
        );

        // The accounting block, every member of it taken from that same result.
        let metadata = run.get("executionMetadata").cloned().unwrap_or(Value::Null);
        for counter in [
            "bytesProcessed",
            "bytesWritten",
            "recordsProcessed",
            "maxPageSize",
            "numParallelProcesses",
        ] {
            assert_eq!(
                number(&metadata, counter),
                number(&result, counter),
                "case {case:?}: expected executionMetadata.{counter} to be what \
                 executionResult reports ({:?}), got {metadata:?}",
                number(&result, counter)
            );
        }
        let put_failures = result
            .get("errorInfo")
            .and_then(|info| number(info, "putFailuresCount"))
            .unwrap_or(0);
        for counter in ["numFailures", "numPutFailures"] {
            assert_eq!(
                number(&metadata, counter),
                Some(put_failures),
                "case {case:?}: expected executionMetadata.{counter} to be the run's own put \
                 failures ({put_failures}), got {metadata:?}"
            );
        }
        assert_eq!(
            number(&metadata, "numFilteredRecords"),
            Some(0),
            "case {case:?}: no record the run read was filtered out, got {metadata:?}"
        );
        assert_eq!(
            metadata.get("terminateFlow").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: no run asks for its flow to be terminated, got {metadata:?}"
        );
        let expected_reason = result
            .get("errorInfo")
            .and_then(|info| info.get("executionMessage"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            metadata.get("reason").cloned().unwrap_or(Value::Null),
            expected_reason,
            "case {case:?}: expected the reason the result carries ({expected_reason:?}), got \
             {metadata:?}"
        );

        // Both names of each of the two instants, carrying the one stored value.
        for (modelled, reported) in [
            ("startedAt", "executionStartTime"),
            ("lastUpdatedAt", "lastUpdatedTime"),
        ] {
            let at = instant(&run, modelled);
            assert!(
                at.is_some_and(|at| at > 0.0),
                "case {case:?}: expected {modelled} to carry the instant it names, got {run:?}"
            );
            assert_eq!(
                instant(&run, reported),
                at,
                "case {case:?}: expected {reported} to carry the same instant as {modelled} \
                 ({at:?}), got {run:?}"
            );
        }

        // The data-pull window, reported only by a run that queried for one.
        let pulled = row.datetime_field.is_some() && !row.unreachable_source;
        let window = (
            instant(&run, "dataPullStartTime"),
            instant(&run, "dataPullEndTime"),
        );
        if pulled {
            match window {
                (Some(start), Some(end)) => assert!(
                    start > 0.0 && end >= start,
                    "case {case:?}: expected the data-pull window to begin before it ends, \
                     got {window:?}"
                ),
                _ => panic!(
                    "case {case:?}: expected a run that pulled new or updated records to \
                     report the window it pulled over, got {run:?}"
                ),
            }
        } else {
            assert_eq!(
                window,
                (None, None),
                "case {case:?}: expected no data-pull window for a run that queried for \
                 none, got {run:?}"
            );
        }
    }
}

/// A `Map_all` task, which maps every field of every record the run reads.
fn map_all() -> Value {
    json!({ "taskType": "Map_all", "sourceFields": [], "taskProperties": {} })
}

/// One row of the data-pull declaration table.
struct Declaration {
    case: &'static str,
    /// The datetime field `incrementalPullConfig` names, when the definition carries one.
    datetime_field: Option<&'static str>,
    /// The `dataPullMode` of a scheduled trigger, when the definition carries one.
    data_pull_mode: Option<&'static str>,
    pulls_incrementally: bool,
}

/// Whether a run of a flow pulls a WINDOW of new or updated records follows from the
/// flow's definition, in either of the two places the model declares it.
///
/// The `dataPullMode` values are the model's own
/// (`smithy_query(query="shape", shape="DataPullMode")`: `Incremental`, `Complete`), so the
/// table names both and not only the one a fixture happens to use.
fn declarations() -> Vec<Declaration> {
    vec![
        Declaration {
            case: "a definition that declares neither",
            datetime_field: None,
            data_pull_mode: None,
            pulls_incrementally: false,
        },
        Declaration {
            case: "an incrementalPullConfig naming no datetime field",
            datetime_field: Some(""),
            data_pull_mode: None,
            pulls_incrementally: false,
        },
        Declaration {
            case: "an incrementalPullConfig naming a datetime field",
            datetime_field: Some("LastModifiedDate"),
            data_pull_mode: None,
            pulls_incrementally: true,
        },
        Declaration {
            case: "a schedule whose every run transfers everything",
            datetime_field: None,
            data_pull_mode: Some("Complete"),
            pulls_incrementally: false,
        },
        Declaration {
            case: "a schedule whose every run transfers what changed",
            datetime_field: None,
            data_pull_mode: Some("Incremental"),
            pulls_incrementally: true,
        },
        Declaration {
            case: "both, which is still one incremental pull",
            datetime_field: Some("SystemModstamp"),
            data_pull_mode: Some("Incremental"),
            pulls_incrementally: true,
        },
    ]
}

#[test]
fn an_incremental_pull_is_the_one_thing_that_gives_a_run_a_data_pull_window() {
    for row in declarations() {
        let case = row.case;
        let mut flow = stored_flow("declaration-flow");
        flow.source_flow_config = SourceFlowConfig {
            incremental_pull_config: row.datetime_field.map(|field| IncrementalPullConfig {
                datetime_type_field_name: Some(field.to_string()),
            }),
            ..SourceFlowConfig::default()
        };
        flow.trigger_config = TriggerConfig {
            trigger_properties: row.data_pull_mode.map(|mode| TriggerProperties {
                scheduled: Some(ScheduledTriggerProperties {
                    schedule_expression: "rate(1hours)".to_string(),
                    data_pull_mode: Some(mode.to_string()),
                    ..ScheduledTriggerProperties::default()
                }),
            }),
            ..TriggerConfig::default()
        };

        assert_eq!(
            flow.pulls_incrementally(),
            row.pulls_incrementally,
            "case {case:?}: expected pulls_incrementally to be {}, got {}",
            row.pulls_incrementally,
            flow.pulls_incrementally()
        );
    }
}

/// One row of the projection table: a run in some state, and what its record says.
struct Projected {
    case: &'static str,
    status: ExecutionStatus,
    result: Option<ExecutionResult>,
    /// The sentence the record is expected to carry, if any.
    expected_message: Option<&'static str>,
    /// Whether the record is expected to carry an accounting block at all.
    expects_metadata: bool,
    /// The reason the accounting block is expected to carry, if any.
    expected_reason: Option<&'static str>,
}

/// The `executionResult` of a run that moved something.
fn transferred(records: i64, bytes: i64) -> ExecutionResult {
    ExecutionResult {
        error_info: None,
        bytes_processed: Some(bytes),
        bytes_written: Some(bytes),
        records_processed: Some(records),
        num_parallel_processes: Some(1),
        max_page_size: Some(0),
    }
}

/// The `executionResult` of a run that failed, carrying why.
fn refused(reason: &str) -> ExecutionResult {
    ExecutionResult {
        error_info: Some(ErrorInfo {
            put_failures_count: Some(0),
            execution_message: Some(reason.to_string()),
        }),
        bytes_processed: Some(0),
        bytes_written: Some(0),
        records_processed: Some(0),
        num_parallel_processes: Some(0),
        max_page_size: Some(0),
    }
}

/// The reason the failing row below carries: a sentence inside
/// `ErrorInfo$executionMessage`'s pattern.
const DESTINATION_REFUSED: &str = "The destination refused the records the run read.";

fn projections() -> Vec<Projected> {
    vec![
        Projected {
            case: "a run that has not finished",
            status: ExecutionStatus::InProgress,
            result: None,
            expected_message: None,
            expects_metadata: false,
            expected_reason: None,
        },
        Projected {
            case: "a run that succeeded",
            status: ExecutionStatus::Successful,
            result: Some(transferred(3, 112)),
            expected_message: Some(RUN_SUCCEEDED_MESSAGE),
            expects_metadata: true,
            expected_reason: None,
        },
        Projected {
            case: "a run that failed",
            status: ExecutionStatus::Error,
            result: Some(refused(DESTINATION_REFUSED)),
            expected_message: Some(DESTINATION_REFUSED),
            expects_metadata: true,
            expected_reason: Some(DESTINATION_REFUSED),
        },
        Projected {
            case: "a run that was cancelled",
            status: ExecutionStatus::Canceled,
            result: Some(transferred(0, 0)),
            expected_message: None,
            expects_metadata: true,
            expected_reason: None,
        },
    ]
}

#[test]
fn the_record_of_a_run_reports_what_that_run_holds_and_nothing_else() {
    for row in projections() {
        let case = row.case;
        let started_at = SystemTime::now();
        let execution = FlowExecution {
            execution_id: "projected-run".to_string(),
            client_token: None,
            execution_status: row.status,
            execution_result: row.result.clone(),
            started_at,
            last_updated_at: started_at + Duration::from_millis(500),
            settles_at: None,
            outcome: None,
            data_pull_start_time: None,
            data_pull_end_time: None,
        };
        let record = execution.to_execution_record();

        assert_eq!(
            record.execution_message.as_deref(),
            row.expected_message,
            "case {case:?}: expected {:?} as the run's message, got {:?}",
            row.expected_message,
            record.execution_message
        );
        assert_eq!(
            record.execution_metadata.is_some(),
            row.expects_metadata,
            "case {case:?}: expected an accounting block to be {}, got {:?}",
            if row.expects_metadata {
                "reported"
            } else {
                "absent"
            },
            record.execution_metadata
        );
        if let (Some(metadata), Some(result)) = (&record.execution_metadata, &row.result) {
            assert_eq!(
                (
                    metadata.bytes_processed,
                    metadata.bytes_written,
                    metadata.records_processed,
                    metadata.max_page_size,
                    metadata.num_parallel_processes
                ),
                (
                    result.bytes_processed,
                    result.bytes_written,
                    result.records_processed,
                    result.max_page_size,
                    result.num_parallel_processes
                ),
                "case {case:?}: expected the accounting block to carry the result's own \
                 counters, got {metadata:?}"
            );
            assert_eq!(
                metadata.reason.as_deref(),
                row.expected_reason,
                "case {case:?}: expected {:?} as the accounting block's reason, got {:?}",
                row.expected_reason,
                metadata.reason
            );
        }

        // The two instants, under both of the names each is reported under.
        assert_eq!(
            record.execution_start_time.map(|at| at.as_secs_f64()),
            record.started_at.map(|at| at.as_secs_f64()),
            "case {case:?}: expected executionStartTime to be startedAt, got {record:?}"
        );
        assert_eq!(
            record.last_updated_time.map(|at| at.as_secs_f64()),
            record.last_updated_at.map(|at| at.as_secs_f64()),
            "case {case:?}: expected lastUpdatedTime to be lastUpdatedAt, got {record:?}"
        );
    }
}
