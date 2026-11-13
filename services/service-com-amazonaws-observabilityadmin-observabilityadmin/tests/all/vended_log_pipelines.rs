// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the pipelines that read log events AWS itself VENDS: what
//! `ValidateTelemetryPipelineConfiguration` reports about their processors, and how a
//! read names their data source.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through the
//! ONE harness this test crate has (`telemetry_rules`), so what is asserted is what a
//! signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. a pipeline section reading from an AWS-vended log source runs a RESTRICTED
//!    processor set — one branch of the schema per source type, and per vended data
//!    source for a source type whose restriction is per data source — so a section that
//!    violates a branch is reported one defect for that branch, under the restriction's
//!    own category and keyed by the restriction's descriptors; a section that declares no
//!    source pins no branch, so every branch it violates is reported, and a section
//!    reading from a third-party source carries none of these restrictions;
//! 2. a POSITION of such a section's processor sequence naming a processor that branch of
//!    the schema does not define at all is reported as the undefined property it is, at
//!    that position's path;
//! 3. a read of a pipeline whose vended source declares no data source names that data
//!    source the way the log event metadata names one — the pipeline's name as an
//!    IDENTIFIER, which separates its words with `_` where the resource name separates
//!    them with `-` — while a data source the document DOES declare is reported exactly
//!    as declared.

use serde_json::{json, Value};

use crate::telemetry_rules::{call, emulator, Emulator};

/// The region every request below signs for.
const HOME_REGION: &str = "us-west-2";

/// The category a defect of one of the schema's vended-log restrictions carries, and the
/// category a position naming a processor the schema does not define carries.
const RESTRICTED: &str = "DATA_SOURCE_PROCESSOR_RESTRICTED";
const UNKNOWN_PROPERTY: &str = "UNKNOWN_PROPERTY";

/// The `FieldMap` key a defect pinned to a place in the document carries.
const PATH_KEY: &str = "path";

/// One `ValidateTelemetryPipelineConfiguration` call.
async fn validate(emulator: &Emulator, configuration: &str) -> (u16, Value) {
    let (status, response) = call(
        emulator,
        HOME_REGION,
        "/ValidateTelemetryPipelineConfiguration",
        &json!({ "Configuration": { "Body": configuration } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a configuration's defects are REPORTED, so the call answers 200, got {status} \
         {response:?}"
    );
    (status, response)
}

/// Every reported defect of `reason`, as `(Message, the FieldMap's key/value pairs)`
/// sorted — so a case asserts the SET of defects rather than the order the schema's
/// branches were walked in.
fn reported(case: &str, response: &Value, reason: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut found: Vec<(String, Vec<(String, String)>)> = response
        .get("Errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|error| error.get("Reason").and_then(Value::as_str) == Some(reason))
        .map(|error| {
            let message = error
                .get("Message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every defect says what is wrong, got {error:?}")
                })
                .to_string();
            let field_map = error
                .get("FieldMap")
                .and_then(Value::as_object)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every defect carries a FieldMap, got {error:?}")
                });
            let mut descriptors: Vec<(String, String)> = field_map
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            descriptors.sort();
            (message, descriptors)
        })
        .collect();
    found.sort();
    found
}

/// One expected defect, written the way [`reported`] reads one back.
fn defect(message: &str, field_map: &[(&str, &str)]) -> (String, Vec<(String, String)>) {
    let mut descriptors: Vec<(String, String)> = field_map
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    descriptors.sort();
    (message.to_string(), descriptors)
}

/// A pipeline definition written at the top level, with the `source` and `processor`
/// text given and one sink.
fn top_level(source: &str, processor: &str) -> String {
    format!(
        "{source}\
         {processor}\
         sink:\n  \
           - cloudwatch_logs:\n      \
               log_group: /aws/telemetry/example\n"
    )
}

// =============================================================================
// Rules 1 and 2 — the vended-log branches of the schema
// =============================================================================

/// A section reading AWS-vended log events is reported one defect per branch of the
/// schema it violates, and one per processor position that branch does not define.
///
/// The rows differ along what the rule generalises over: which source the section
/// declares (none, each of the vended ones, a third-party one), which vended data
/// sources it names (one, none), how many processors it runs (none, one, two, three) and
/// whether the branch admits them, and whether the property a branch's source requires
/// is declared.
#[tokio::test]
async fn a_vended_log_section_is_reported_against_every_branch_it_violates() {
    let emulator = emulator();

    for (case, configuration, expected_restrictions, expected_positions) in [
        (
            "a section declaring no source, running two processors the vended branches \
             do not define",
            "pipeline:\n  processor:\n    - lowercase_string:\n        \
               with_keys: [\"message\"]\n    - uppercase_string:\n        \
               with_keys: [\"message\"]\n"
                .to_string(),
            vec![
                defect(
                    "must have at most 1 items but found 2",
                    &[
                        ("source_type", "cloudwatch_logs"),
                        ("data_source_name", "aws_cloudtrail"),
                        ("allowed_processor", "ocsf"),
                    ],
                ),
                defect(
                    "must have at most 1 items but found 2",
                    &[
                        ("source_type", "cloudwatch_logs"),
                        ("data_source_name", "aws_security_hub_cspm"),
                        ("allowed_processor", "ocsf"),
                    ],
                ),
                defect(
                    "property 'lowercase_string' is not defined in the schema and the \
                     schema does not allow additional properties",
                    &[
                        ("source_type", "amazon_vpc_flow"),
                        ("allowed_processors", "parse_vpc/ocsf"),
                    ],
                ),
                defect(
                    "property 'lowercase_string' is not defined in the schema and the \
                     schema does not allow additional properties",
                    &[
                        ("source_type", "amazon_route53"),
                        ("allowed_processors", "parse_route53/ocsf"),
                    ],
                ),
                defect(
                    "required property 'cloudfront' not found",
                    &[
                        ("source_type", "amazon_cloudfront"),
                        ("allowed_processors", "parse_cloudfront/ocsf"),
                    ],
                ),
            ],
            vec![
                defect(
                    "property 'lowercase_string' is not defined in the schema and the \
                     schema does not allow additional properties",
                    &[(PATH_KEY, "processor[0]")],
                ),
                defect(
                    "property 'uppercase_string' is not defined in the schema and the \
                     schema does not allow additional properties",
                    &[(PATH_KEY, "processor[1]")],
                ),
            ],
        ),
        (
            "a vended data source running a processor its own branch does not admit",
            top_level(
                "source:\n  cloudwatch_logs:\n    data_sources:\n      \
                   - aws_security_hub_cspm\n",
                "processor:\n  - parse_json: {}\n",
            ),
            vec![defect(
                "property 'parse_json' is not defined in the schema and the schema does \
                 not allow additional properties",
                &[
                    ("source_type", "cloudwatch_logs"),
                    ("data_source_name", "aws_security_hub_cspm"),
                    ("allowed_processor", "ocsf"),
                ],
            )],
            Vec::new(),
        ),
        (
            "a vended data source running exactly the processor its branch admits",
            top_level(
                "source:\n  cloudwatch_logs:\n    data_sources:\n      \
                   - aws_cloudtrail\n",
                "processor:\n  - ocsf: {}\n",
            ),
            Vec::new(),
            Vec::new(),
        ),
        (
            "a vended source running more processors than its branch admits",
            top_level(
                "source:\n  amazon_route53: {}\n",
                "processor:\n  - parse_route53: {}\n  - ocsf: {}\n  - parse_json: {}\n",
            ),
            vec![defect(
                "must have at most 2 items but found 3",
                &[
                    ("source_type", "amazon_route53"),
                    ("allowed_processors", "parse_route53/ocsf"),
                ],
            )],
            Vec::new(),
        ),
        (
            "a vended source declaring the property its branch requires, with a \
             processor that branch admits",
            top_level(
                "source:\n  amazon_cloudfront:\n    cloudfront:\n      \
                   distribution_id: E1EXAMPLE\n",
                "processor:\n  - parse_cloudfront: {}\n",
            ),
            Vec::new(),
            Vec::new(),
        ),
        (
            "a vended source leaving out the property its branch requires",
            top_level(
                "source:\n  amazon_cloudfront: {}\n",
                "processor:\n  - parse_cloudfront: {}\n",
            ),
            vec![defect(
                "required property 'cloudfront' not found",
                &[
                    ("source_type", "amazon_cloudfront"),
                    ("allowed_processors", "parse_cloudfront/ocsf"),
                ],
            )],
            Vec::new(),
        ),
        (
            "a vended source running the one processor its branch admits",
            top_level(
                "source:\n  amazon_vpc_flow: {}\n",
                "processor:\n  - parse_vpc: {}\n",
            ),
            Vec::new(),
            Vec::new(),
        ),
        (
            "a third-party source, which carries none of these restrictions",
            top_level(
                "source:\n  http: {}\n",
                "processor:\n  - uppercase_string:\n      with_keys: [\"message\"]\n",
            ),
            Vec::new(),
            Vec::new(),
        ),
        (
            "a section declaring no source and no processor, which restricts nothing",
            "pipeline:\n  sink:\n    - cloudwatch_logs: {}\n".to_string(),
            Vec::new(),
            Vec::new(),
        ),
    ] {
        let (_, response) = validate(&emulator, &configuration).await;
        let mut expected_restrictions = expected_restrictions;
        expected_restrictions.sort();
        assert_eq!(
            reported(case, &response, RESTRICTED),
            expected_restrictions,
            "case {case:?}: expected those restriction defects for {configuration:?}, \
             got {response:?}"
        );
        let mut expected_positions = expected_positions;
        expected_positions.sort();
        assert_eq!(
            reported(case, &response, UNKNOWN_PROPERTY),
            expected_positions,
            "case {case:?}: expected those undefined-processor defects for \
             {configuration:?}, got {response:?}"
        );
    }
}

// =============================================================================
// Rule 3 — how a read names a vended source's own data source
// =============================================================================

/// A vended source that declares no data source reads the events vended for the pipeline
/// itself, so the read names that data source with the pipeline's name in IDENTIFIER
/// form; a data source the document declares is reported exactly as declared.
///
/// The rows differ along what the rule generalises over: how many words the pipeline's
/// name separates (none, two, several), which vended source reads them, whether the
/// document declares the data source at all, and whether a declared name itself carries
/// the resource separator.
#[tokio::test]
async fn a_vended_sources_own_data_source_is_named_by_the_pipelines_identifier() {
    let emulator = emulator();

    for (case, name, body, expected) in [
        (
            "a CloudWatch Logs source declaring no data source",
            "probe-pipe-one",
            "source:\n  cloudwatch_logs:\n    log_group: /aws/telemetry/in\n\
             sink:\n  - cloudwatch_logs: {}\n",
            json!([{ "Name": "probe_pipe_one", "Type": "default" }]),
        ),
        (
            "a pipeline name that separates no words at all",
            "probepipeline",
            "source:\n  cloudwatch_logs: {}\nsink:\n  - cloudwatch_logs: {}\n",
            json!([{ "Name": "probepipeline", "Type": "default" }]),
        ),
        (
            "a pipeline name separating several words",
            "sim2real-probe-pipe-a1b2",
            "pipeline:\n  source:\n    cloudwatch_logs: {}\n  \
             sink:\n    - cloudwatch_logs: {}\n",
            json!([{ "Name": "sim2real_probe_pipe_a1b2", "Type": "default" }]),
        ),
        (
            "another vended source declaring no data source",
            "flow-logs-pipe",
            "source:\n  amazon_vpc_flow: {}\nsink:\n  - cloudwatch_logs: {}\n",
            json!([{ "Name": "flow_logs_pipe", "Type": "default" }]),
        ),
        (
            "a vended source declaring a data source whose own name carries the \
             resource separator",
            "declared-feed-pipe",
            "source:\n  cloudwatch_logs:\n    data_sources:\n      - custom-feed\n\
             sink:\n  - cloudwatch_logs: {}\n",
            json!([{ "Name": "custom-feed", "Type": "default" }]),
        ),
        (
            "a third-party source declaring no data source, which defaults to the \
             plugin name",
            "third-party-pipe",
            "source:\n  s3:\n    bucket: telemetry-archive\n\
             sink:\n  - cloudwatch_logs: {}\n",
            json!([{ "Name": "s3" }]),
        ),
    ] {
        let (status, created) = call(
            &emulator,
            HOME_REGION,
            "/CreateTelemetryPipeline",
            &json!({ "Name": name, "Configuration": { "Body": body } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: creating {name:?} must answer 200, got {status} {created:?}"
        );

        let (status, read) = call(
            &emulator,
            HOME_REGION,
            "/GetTelemetryPipeline",
            &json!({ "PipelineIdentifier": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading {name:?} must answer 200, got {status} {read:?}"
        );
        assert_eq!(
            read.pointer("/Pipeline/ConfigurationSummary/DataSources"),
            Some(&expected),
            "case {case:?}: expected those data sources for {body:?}, got {read:?}"
        );
        // The identifier form is the DATA SOURCE's name only: the pipeline resource
        // keeps the name it was created with, which is the one a later read takes.
        assert_eq!(
            read.pointer("/Pipeline/Name").and_then(Value::as_str),
            Some(name),
            "case {case:?}: the pipeline keeps its own name, got {read:?}"
        );
    }
}
