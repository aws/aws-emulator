// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the pipeline projection `GetTelemetryPipeline` answers:
//! `Pipeline.ConfigurationSummary` and `Pipeline.StatusReason`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through the
//! ONE harness this test crate has (`telemetry_rules`), so what is asserted is what a
//! signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. a read of a pipeline reports the components of the configuration document that
//!    pipeline was created with: one `Sources` entry per source plugin the document
//!    declares, one `DataSources` entry per data source those sources declare, the
//!    processor names in document order, their count, and the sink plugin names — so the
//!    summary is DERIVED from the stored body, never from the request that read it;
//! 2. a source declaring no data source still has one, named the way
//!    `DataSource$Name`'s documentation says that kind of source names it;
//! 3. `ProcessorCount` is the length of `Processors`, whatever the document declares;
//! 4. a body the configuration reader cannot read declares no components, so such a
//!    pipeline carries no summary at all;
//! 5. the status reason reports the state the pipeline is IN: an `ACTIVE` pipeline
//!    answers AWS's fixed sentence for that status, and the summary is the same before
//!    and after the transition, because it comes from the document and not from the
//!    clock.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::PIPELINE_ACTIVATION_DELAY_MS;

use crate::telemetry_rules::{call, emulator, Emulator};

/// The region every request below signs for.
const HOME_REGION: &str = "us-west-2";

/// The `StatusReason$Description` an ACTIVE pipeline reports: wholly fixed text, as the
/// recorded response words it.
const ACTIVE_STATUS_REASON: &str = "Pipeline is active and processing data";

/// The `StatusReason$Description` a pipeline whose creation has not come due yet
/// reports. No response for that status is recorded, so this one is the emulator's own
/// sentence in the same form; what the rule requires is that it is the sentence for the
/// status the pipeline is IN, which is a different one from the ACTIVE sentence.
const CREATING_STATUS_REASON: &str = "Pipeline is being created";

/// Long enough after a create for the `CREATING` -> `ACTIVE` transition to have come
/// due, whatever scheduling delay the test run adds on top.
fn past_activation_delay() -> Duration {
    Duration::from_millis(PIPELINE_ACTIVATION_DELAY_MS as u64 + 250)
}

/// Create the pipeline `name` with `body` and read it back, answering the read's
/// `Pipeline`.
async fn create_and_read(emulator: &Emulator, name: &str, body: &str) -> Value {
    let (status, created) = call(
        emulator,
        HOME_REGION,
        "/CreateTelemetryPipeline",
        &json!({ "Name": name, "Configuration": { "Body": body } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating {name:?} must answer 200, got {status} {created:?}"
    );

    let (status, read) = call(
        emulator,
        HOME_REGION,
        "/GetTelemetryPipeline",
        &json!({ "PipelineIdentifier": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "reading {name:?} must answer 200, got {status} {read:?}"
    );
    read.pointer("/Pipeline")
        .cloned()
        .unwrap_or_else(|| panic!("the read reports the pipeline, got {read:?}"))
}

// =============================================================================
// Rules 1 and 3 — the summary is derived from the stored configuration document
// =============================================================================

/// The summary a read reports is the one the stored document declares.
///
/// The rows differ along what the derivation generalises over: where the document writes
/// its pipeline (at the top level, under the `pipeline` property the schema requires, as
/// a chain of two named sections), how many processors it declares (none, one, several)
/// and in which order, how its data sources are declared (as a list of names, as a list
/// of name/type mappings, as the `data_source_name`/`data_source_type` pair the log
/// event metadata names them by, not at all), how many of them there are, and how the
/// document is spelled (YAML or the JSON it is a superset of).
#[tokio::test]
async fn a_read_reports_the_components_the_stored_document_declares() {
    let emulator = emulator();

    for (index, (case, body, expected)) in [
        (
            "a pipeline under the document's own `pipeline` property, naming one data \
             source",
            "version: \"2\"\npipeline:\n  source:\n    cloudwatch_logs:\n      \
               data_sources:\n        - name: aws_cloudtrail\n          type: default\n  \
             processor:\n    - parse_json: {}\n  \
             sink:\n    - cloudwatch_logs:\n        log_group: /aws/telemetry/out\n"
                .to_string(),
            json!({
                "Sources": [{ "Type": "cloudwatch_logs" }],
                "DataSources": [{ "Name": "aws_cloudtrail", "Type": "default" }],
                "Processors": ["parse_json"],
                "ProcessorCount": 1,
                "Sinks": ["cloudwatch_logs"],
            }),
        ),
        (
            "two data sources listed by name only, each taking the default type",
            "pipeline:\n  source:\n    cloudwatch_logs:\n      data_sources:\n        \
               - aws_cloudtrail\n        - aws_security_hub_cspm\n  \
             sink:\n    - cloudwatch_logs: {}\n"
                .to_string(),
            json!({
                "Sources": [{ "Type": "cloudwatch_logs" }],
                "DataSources": [
                    { "Name": "aws_cloudtrail", "Type": "default" },
                    { "Name": "aws_security_hub_cspm", "Type": "default" },
                ],
                "Processors": [],
                "ProcessorCount": 0,
                "Sinks": ["cloudwatch_logs"],
            }),
        ),
        (
            "a source naming its data source by the pair the log metadata names",
            "source:\n  s3:\n    data_source_name: archive-logs\n    \
               data_source_type: custom\n\
             sink:\n  - cloudwatch_logs: {}\n"
                .to_string(),
            json!({
                "Sources": [{ "Type": "s3" }],
                "DataSources": [{ "Name": "archive-logs", "Type": "custom" }],
                "Processors": [],
                "ProcessorCount": 0,
                "Sinks": ["cloudwatch_logs"],
            }),
        ),
        (
            "several processors, reported in document order and counted",
            "source:\n  http: {}\n\
             processor:\n  \
               - parse_json: {}\n  \
               - trim_string:\n      with_keys: [\"message\"]\n  \
               - uppercase_string:\n      with_keys: [\"message\"]\n\
             sink:\n  - s3:\n      bucket: telemetry-archive\n"
                .to_string(),
            json!({
                "Sources": [{ "Type": "http" }],
                "DataSources": [{ "Name": "http" }],
                "Processors": ["parse_json", "trim_string", "uppercase_string"],
                "ProcessorCount": 3,
                "Sinks": ["s3"],
            }),
        ),
        (
            "a sink written as a mapping of one plugin rather than a sequence",
            "source:\n  http: {}\nsink:\n  cloudwatch_logs:\n    log_group: /aws/x\n".to_string(),
            json!({
                "Sources": [{ "Type": "http" }],
                "DataSources": [{ "Name": "http" }],
                "Processors": [],
                "ProcessorCount": 0,
                "Sinks": ["cloudwatch_logs"],
            }),
        ),
        (
            "the same pipeline written as JSON, which YAML 1.2 is a superset of",
            json!({
                "pipeline": {
                    "source": { "cloudwatch_logs": { "data_sources": [{ "name": "aws_waf" }] } },
                    "processor": [{ "parse_json": {} }],
                    "sink": [{ "cloudwatch_logs": { "log_group": "/aws/x" } }],
                }
            })
            .to_string(),
            json!({
                "Sources": [{ "Type": "cloudwatch_logs" }],
                "DataSources": [{ "Name": "aws_waf", "Type": "default" }],
                "Processors": ["parse_json"],
                "ProcessorCount": 1,
                "Sinks": ["cloudwatch_logs"],
            }),
        ),
        (
            "two sections chained by a pipeline sink and a pipeline source",
            "entry:\n  \
               source:\n    http: {}\n  \
               processor:\n    - trim_string:\n        with_keys: [\"message\"]\n  \
               sink:\n    - pipeline:\n        name: exit\n\
             exit:\n  \
               source:\n    pipeline:\n      name: entry\n  \
               processor:\n    - uppercase_string:\n        with_keys: [\"message\"]\n  \
               sink:\n    - cloudwatch_logs:\n        log_group: /aws/x\n"
                .to_string(),
            json!({
                "Sources": [{ "Type": "http" }, { "Type": "pipeline" }],
                "DataSources": [
                    { "Name": "http" },
                    { "Name": "pipeline" },
                ],
                "Processors": ["trim_string", "uppercase_string"],
                "ProcessorCount": 2,
                "Sinks": ["pipeline", "cloudwatch_logs"],
            }),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let pipeline = create_and_read(&emulator, &format!("summary-{index}"), &body).await;
        let summary = pipeline
            .get("ConfigurationSummary")
            .unwrap_or_else(|| panic!("case {case:?}: expected a summary, got {pipeline:?}"));
        assert_eq!(
            summary, &expected,
            "case {case:?}: expected the summary of {body:?}, got {summary:?}"
        );
        let processors = summary
            .get("Processors")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            summary.get("ProcessorCount").and_then(Value::as_i64),
            Some(processors as i64),
            "case {case:?}: ProcessorCount is the length of Processors, got {summary:?}"
        );
        assert_eq!(
            pipeline
                .pointer("/Configuration/Body")
                .and_then(Value::as_str),
            Some(body.as_str()),
            "case {case:?}: the summary is reported beside the document it came from, \
             got {pipeline:?}"
        );
    }
}

// =============================================================================
// Rule 2 — a source declaring no data source still has one
// =============================================================================

/// A source that declares no data source still has one, and a data source that declares
/// no type may still report one: `DataSource$Name`'s documentation splits both defaults
/// by the KIND of source. A source reading log events AWS vends reads the events vended
/// for the pipeline itself, so its data source is the pipeline, under the pipeline's own
/// name and the default type; a third-party source "defaults to the plugin name if not
/// specified", "with the type field left empty".
///
/// The rows differ along what the rule generalises over: the kind of source (two
/// AWS-vended ones, two different third-party ones), where the document writes the
/// pipeline, how much of the data source the document declares (nothing, its name only,
/// name and type), and how many it declares.
#[tokio::test]
async fn a_source_declaring_no_data_source_is_named_the_way_its_kind_names_one() {
    let emulator = emulator();

    for (index, (case, body, expected)) in [
        (
            "a CloudWatch Logs source at the top level, declaring no data source",
            "source:\n  cloudwatch_logs:\n    log_group: /aws/telemetry/example\n\
             processor:\n  - parse_json: {}\n\
             sink:\n  - cloudwatch_logs:\n      log_group: /aws/telemetry/out\n",
            None,
        ),
        (
            "a CloudWatch Logs source under the document's `pipeline` property, declaring \
             no data source",
            "version: \"2\"\npipeline:\n  source:\n    cloudwatch_logs: {}\n  \
             sink:\n    - cloudwatch_logs: {}\n",
            None,
        ),
        (
            "another AWS-vended source, declaring no data source",
            "pipeline:\n  source:\n    amazon_vpc_flow: {}\n  \
             sink:\n    - cloudwatch_logs: {}\n",
            None,
        ),
        (
            "a third-party s3 source, declaring no data source",
            "source:\n  s3:\n    bucket: telemetry-archive\n\
             sink:\n  - cloudwatch_logs: {}\n",
            Some(json!([{ "Name": "s3" }])),
        ),
        (
            "a third-party http source, declaring no data source",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n",
            Some(json!([{ "Name": "http" }])),
        ),
        (
            "a third-party s3 source naming its data source but not its type",
            "source:\n  s3:\n    data_source_name: archive-logs\n\
             sink:\n  - cloudwatch_logs: {}\n",
            Some(json!([{ "Name": "archive-logs" }])),
        ),
        (
            "a third-party s3 source listing two data sources by name only",
            "source:\n  s3:\n    data_sources:\n      - archive-logs\n      - audit-logs\n\
             sink:\n  - cloudwatch_logs: {}\n",
            Some(json!([{ "Name": "archive-logs" }, { "Name": "audit-logs" }])),
        ),
        (
            "a CloudWatch Logs source that DOES declare its data source",
            "pipeline:\n  source:\n    cloudwatch_logs:\n      data_sources:\n        \
               - aws_cloudtrail\n  sink:\n    - cloudwatch_logs: {}\n",
            Some(json!([{ "Name": "aws_cloudtrail", "Type": "default" }])),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("default-source-{index}");
        let pipeline = create_and_read(&emulator, &name, body).await;
        // A CloudWatch Logs source's data source is the pipeline itself, named the way
        // the log event metadata names one: an identifier, which separates its words
        // with `_` where the resource name separates them with `-`. So the row expects
        // THIS pipeline's name in that form rather than a literal.
        let expected = expected
            .unwrap_or_else(|| json!([{ "Name": name.replace('-', "_"), "Type": "default" }]));
        assert_eq!(
            pipeline.pointer("/ConfigurationSummary/DataSources"),
            Some(&expected),
            "case {case:?}: expected those data sources for {body:?}, got {pipeline:?}"
        );
    }
}

// =============================================================================
// Rule 4 — a document the reader cannot read declares no components
// =============================================================================

/// A stored body the configuration reader cannot read has no components to summarise, so
/// the read reports no summary rather than an empty one: a document that was never
/// parsed is a different observation from one declaring nothing.
///
/// The rows span the ways a body is unreadable, the same ones
/// `ValidateTelemetryPipelineConfiguration` reports as parse errors.
#[tokio::test]
async fn a_body_the_reader_cannot_read_has_no_summary() {
    let emulator = emulator();

    for (index, (case, body)) in [
        (
            "a mapping entry followed by a sequence entry at the same level",
            "source:\n  http: {}\n- broken\n".to_string(),
        ),
        (
            "an unterminated flow sequence",
            "source:\n  http: {}\nsink: [cloudwatch_logs\n".to_string(),
        ),
        (
            "an alias, which expands the document it appears in",
            "base: &anchor\n  http: {}\nsource: *anchor\nsink:\n  - cloudwatch_logs: {}\n"
                .to_string(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let pipeline = create_and_read(&emulator, &format!("unreadable-{index}"), &body).await;
        assert_eq!(
            pipeline.get("ConfigurationSummary"),
            None,
            "case {case:?}: an unreadable document has no summary, got {pipeline:?}"
        );
        assert_eq!(
            pipeline
                .pointer("/Configuration/Body")
                .and_then(Value::as_str),
            Some(body.as_str()),
            "case {case:?}: the body is still reported verbatim, got {pipeline:?}"
        );
    }
}

// =============================================================================
// Rule 5 — the status reason reports the state the pipeline is in
// =============================================================================

/// An `ACTIVE` pipeline reports AWS's sentence for that status, and a pipeline still
/// `CREATING` reports a different one — while the summary, which comes from the stored
/// document rather than from the clock, is the same on both sides of the transition.
#[tokio::test]
async fn the_status_reason_reports_the_status_and_the_summary_does_not_move() {
    let emulator = emulator();
    let body = "pipeline:\n  source:\n    cloudwatch_logs:\n      data_sources:\n        \
                  - name: aws_cloudtrail\n  \
                processor:\n    - parse_json: {}\n  \
                sink:\n    - cloudwatch_logs:\n        log_group: /aws/x\n";

    let creating = create_and_read(&emulator, "status-reason", body).await;
    let creating_status = creating
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let creating_reason = creating
        .pointer("/StatusReason/Description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // Whichever side of the transition this read landed on, the reason is that status's
    // own sentence — so the assertion cannot pass vacuously on a slow run.
    let expected_reason = if creating_status == "CREATING" {
        CREATING_STATUS_REASON
    } else {
        ACTIVE_STATUS_REASON
    };
    assert_eq!(
        creating_reason, expected_reason,
        "case {creating_status:?}: the reason describes the status the pipeline is in, \
         got {creating:?}"
    );

    tokio::time::sleep(past_activation_delay()).await;

    let (status, read) = call(
        &emulator,
        HOME_REGION,
        "/GetTelemetryPipeline",
        &json!({ "PipelineIdentifier": "status-reason" }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {read:?}");
    let active = read
        .pointer("/Pipeline")
        .cloned()
        .unwrap_or_else(|| panic!("the read reports the pipeline, got {read:?}"));
    assert_eq!(
        active.get("Status").and_then(Value::as_str),
        Some("ACTIVE"),
        "once the transition comes due the pipeline is ACTIVE, got {active:?}"
    );
    assert_eq!(
        active
            .pointer("/StatusReason/Description")
            .and_then(Value::as_str),
        Some(ACTIVE_STATUS_REASON),
        "an ACTIVE pipeline reports that status's own sentence, got {active:?}"
    );
    assert_eq!(
        active.get("ConfigurationSummary"),
        creating.get("ConfigurationSummary"),
        "the summary comes from the document, so the transition leaves it alone: \
         got {active:?} after {creating:?}"
    );
}
