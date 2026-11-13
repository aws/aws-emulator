// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ValidateTelemetryPipelineConfiguration`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handler — through the
//! ONE harness this test crate has (`telemetry_rules`), so what is asserted is what a
//! signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. the operation REPORTS the defects of the configuration it is handed: a 200
//!    carrying one `ValidationError` per defect, each with the category it falls in and
//!    a `FieldMap` naming the field of the document it is about — and a 200 carrying no
//!    `Errors` at all for a configuration this emulator would run;
//! 2. a body that is not a document the emulator parses is the
//!    `PIPELINE_CONFIGURATION_PARSE_ERROR` defect and the only one reported for such a
//!    body, since a body that was never read has no fields to report anything else
//!    against;
//! 3. a document that WAS read is checked against the SCHEMA a pipeline definition
//!    follows, one reported error per problem: a required property the document does not
//!    declare (`MISSING_FIELD`, carrying that property's own NAME — the `pipeline` the
//!    schema requires of a document that declares none, and the `source`/`sink` a
//!    section it does declare is missing), a property the schema does not define
//!    (`UNKNOWN_PROPERTY`, carrying the path it is written at), a property carrying a
//!    number of items the schema does not allow, a POSITION of the processor sequence
//!    this emulator cannot run (its own category, since the document may be one the real
//!    service accepts), and every field naming an ARN carrying one — the IAM role syntax
//!    where the field names the role a plugin assumes. A section reading AWS-vended log
//!    events is checked against the schema's restricted branches for that source as well,
//!    whose own category and descriptors `vended_log_pipelines.rs` asserts;
//! 4. nothing is stored and nothing is created: the call leaves the store exactly as it
//!    found it, so the same call answers the same way twice.

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::ComAmazonawsObservabilityadminObservabilityadminState;

use crate::telemetry_rules::{call, emulator, Emulator};

/// The region every request below signs for.
const HOME_REGION: &str = "us-west-2";

/// `ValidationError$Reason` for a body that could not be parsed as a document at all,
/// for a document that was parsed but declares something a pipeline definition may
/// not, and for a field whose value does not have the form that field takes: the
/// category codes this service answers.
const PARSE_ERROR: &str = "PIPELINE_CONFIGURATION_PARSE_ERROR";
const VALIDATION_ERROR: &str = "PIPELINE_CONFIGURATION_VALIDATION_ERROR";
const INVALID_PATTERN: &str = "INVALID_PATTERN";
/// The category for a processor, or a processor option, this emulator does not run —
/// its own code, because such a document may be one the real service accepts.
const UNSUPPORTED_PROCESSOR: &str = "UNSUPPORTED_PROCESSOR";
/// The two categories a schema validator reports a property problem under: a required
/// property that is not there, and a property the schema does not define.
const MISSING_FIELD: &str = "MISSING_FIELD";
const UNKNOWN_PROPERTY: &str = "UNKNOWN_PROPERTY";
/// The category a defect of one of the schema's vended-log restrictions carries, and the
/// two shapes of `FieldMap` descriptors such a defect names: the restriction about one
/// vended data source names that data source too, where the restriction about a whole
/// source type names the processors it admits as a set. `vended_log_pipelines.rs`
/// asserts their values; here they are the defects a case reports.
const RESTRICTED: &str = "DATA_SOURCE_PROCESSOR_RESTRICTED";
const DATA_SOURCE_DESCRIPTORS: &str = "allowed_processor+data_source_name+source_type";
const SOURCE_TYPE_DESCRIPTORS: &str = "allowed_processors+source_type";
/// How [`defects`] joins the keys of a defect that names more than one.
const FIELD_SEPARATOR: &str = "+";

/// The defects every vended-log branch of the schema reports against a section that
/// declares NO source: such a section pins no branch, so each of the five the schema
/// carries is measured against its processors — two restrictions about a vended data
/// source of `cloudwatch_logs`, and one each about the three source types whose whole
/// log shape is restricted.
fn unpinned_branches() -> Vec<(&'static str, &'static str)> {
    vec![
        (RESTRICTED, DATA_SOURCE_DESCRIPTORS),
        (RESTRICTED, DATA_SOURCE_DESCRIPTORS),
        (RESTRICTED, SOURCE_TYPE_DESCRIPTORS),
        (RESTRICTED, SOURCE_TYPE_DESCRIPTORS),
        (RESTRICTED, SOURCE_TYPE_DESCRIPTORS),
    ]
}

/// One ARN of each form this service's model declares
/// (`smithy_query(query="shape", shape="IamRoleArn,ResourceArn")`): the IAM role a
/// pipeline plugin assumes, and the resource a sink writes to.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/TelemetryPipelineRole";
const BUCKET_ARN: &str = "arn:aws:s3:::telemetry-archive";

/// One `ValidateTelemetryPipelineConfiguration` call.
async fn validate(emulator: &Emulator, configuration: &str) -> (u16, Value) {
    call(
        emulator,
        HOME_REGION,
        "/ValidateTelemetryPipelineConfiguration",
        &json!({ "Configuration": { "Body": configuration } }),
    )
    .await
}

/// The reported defects as `(Reason, its FieldMap's keys)` pairs, sorted so a case
/// asserts the SET of defects rather than the order two independent checks ran in. A
/// defect pinned to one field of the document names that one key; a defect of one of the
/// schema's vended-log restrictions is about the restriction rather than about a field,
/// so it names that restriction's descriptors, joined — their VALUES and messages are
/// asserted in `vended_log_pipelines.rs`, which is where that rule lives.
fn defects(case: &str, response: &Value) -> Vec<(String, String)> {
    let errors = response
        .get("Errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut reported: Vec<(String, String)> = errors
        .iter()
        .map(|error| {
            let message = error
                .get("Message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !message.is_empty(),
                "case {case:?}: every reported defect says what is wrong, got {error:?}"
            );
            let reason = error
                .get("Reason")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported defect carries a Reason, got {error:?}")
                });
            let field_map = error
                .get("FieldMap")
                .and_then(Value::as_object)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported defect carries a FieldMap, got {error:?}")
                });
            assert!(
                !field_map.is_empty(),
                "case {case:?}: a defect names what it is about, got {field_map:?}"
            );
            let mut fields: Vec<&str> = Vec::new();
            for (field, issue) in field_map {
                assert!(
                    issue.as_str().is_some_and(|issue| !issue.is_empty()),
                    "case {case:?}: the FieldMap entry for {field} carries the issue, \
                     got {issue:?}"
                );
                fields.push(field);
            }
            fields.sort_unstable();
            (reason.to_string(), fields.join(FIELD_SEPARATOR))
        })
        .collect();
    reported.sort();
    reported
}

/// A pipeline definition written at the top level, with one source, the `processor`
/// text given, and one sink.
fn top_level(processor: &str) -> String {
    format!(
        "source:\n  \
           http: {{}}\n\
         {processor}\
         sink:\n  \
           - cloudwatch_logs:\n      \
               log_group: /aws/telemetry/example\n"
    )
}

/// The same definition under a section name, which is the other form this format
/// writes a pipeline in.
fn named_section(name: &str, processor: &str) -> String {
    format!(
        "{name}:\n  \
           source:\n    \
             http: {{}}\n\
         {processor}  \
           sink:\n    \
             - cloudwatch_logs:\n        \
                 log_group: /aws/telemetry/example\n"
    )
}

// =============================================================================
// A configuration this emulator would run reports no defect
// =============================================================================

/// A configuration that satisfies every rule a pipeline definition follows answers a
/// 200 with NO `Errors`: the output's list is "a list of validation errors found in
/// the pipeline configuration, if any", so its absence is the verdict "valid".
///
/// The rows differ along what the rule generalises over: where the pipeline is written
/// (at the top level or as a named section), how many processors it declares (none,
/// one, several), how its one source and one sink are written (a mapping, a sequence of
/// one), how the document is spelled (YAML or the JSON it is a superset of), and how
/// many sections the chain spans.
#[tokio::test]
async fn a_configuration_this_emulator_would_run_reports_no_defect() {
    let emulator = emulator();

    for (case, configuration) in [
        (
            "a top-level pipeline with one processor",
            top_level("processor:\n  - parse_json: {}\n"),
        ),
        (
            "a top-level pipeline with no processor, which is optional",
            top_level(""),
        ),
        (
            "a top-level pipeline with several processors",
            top_level(
                "processor:\n  \
                   - parse_json: {}\n  \
                   - trim_string:\n      \
                       with_keys: [\"message\"]\n  \
                   - uppercase_string:\n      \
                       with_keys: [\"message\"]\n",
            ),
        ),
        (
            "a named section",
            named_section("log-pipeline", "  processor:\n    - parse_json: {}\n"),
        ),
        (
            "a sink written as a mapping of one plugin rather than a sequence",
            "source:\n  http: {}\nsink:\n  cloudwatch_logs:\n    log_group: /aws/x\n".to_string(),
        ),
        (
            "a document carrying members beside its one section",
            "version: \"2\"\n".to_string()
                + &named_section("log-pipeline", "  processor:\n    - parse_json: {}\n"),
        ),
        (
            "the same pipeline written as JSON, which YAML 1.2 is a superset of",
            json!({
                "log-pipeline": {
                    "source": { "http": {} },
                    "processor": [{ "parse_json": {} }],
                    "sink": [{ "cloudwatch_logs": { "log_group": "/aws/x" } }],
                }
            })
            .to_string(),
        ),
        (
            "a sink naming the role it assumes by its ARN",
            format!(
                "source:\n  http: {{}}\nsink:\n  - cloudwatch_logs:\n      log_group: /aws/x\n      \
                 sts_role_arn: \"{ROLE_ARN}\"\n"
            ),
        ),
        (
            "a source naming its role under the aws block, nested deeper in the document",
            format!(
                "source:\n  s3:\n    aws:\n      sts_role_arn: \"{ROLE_ARN}\"\n      \
                 region: us-west-2\nsink:\n  - cloudwatch_logs:\n      log_group: /aws/x\n"
            ),
        ),
        (
            "a sink naming the resource it writes to by its ARN, which is not a role",
            format!(
                "source:\n  http: {{}}\nsink:\n  - s3:\n      bucket_arn: \"{BUCKET_ARN}\"\n      \
                 sts_role_arn: \"{ROLE_ARN}\"\n"
            ),
        ),
        (
            "the same role field spelled the way JSON writes it",
            format!(
                "{{\"source\": {{\"http\": {{}}}}, \"sink\": [{{\"s3\": \
                 {{\"stsRoleArn\": \"{ROLE_ARN}\"}}}}]}}"
            ),
        ),
        (
            "two sections chained by a pipeline sink and a pipeline source",
            "entry:\n  \
               source:\n    \
                 http: {}\n  \
               processor:\n    \
                 - trim_string:\n        \
                     with_keys: [\"message\"]\n  \
               sink:\n    \
                 - pipeline:\n        \
                     name: exit\n\
             exit:\n  \
               source:\n    \
                 pipeline:\n      \
                   name: entry\n  \
               processor:\n    \
                 - uppercase_string:\n        \
                     with_keys: [\"message\"]\n  \
               sink:\n    \
                 - cloudwatch_logs:\n        \
                     log_group: /aws/x\n"
                .to_string(),
        ),
    ] {
        let (status, response) = validate(&emulator, &configuration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        assert_eq!(
            defects(case, &response),
            Vec::<(String, String)>::new(),
            "case {case:?}: expected no defect for {configuration}, got {response:?}"
        );
    }
}

// =============================================================================
// A body that is not a document the emulator reads
// =============================================================================

/// A body the reader cannot parse is reported as the `PIPELINE_CONFIGURATION_PARSE_ERROR`
/// defect against `Configuration.Body`, and as that ONE defect: nothing was read, so
/// there is no field in the document to report anything else against.
///
/// The rows span the ways a body is unreadable: a syntax error, an unterminated
/// collection, an alias (which expands the document it appears in), and nesting past
/// what the reader reads.
#[tokio::test]
async fn a_body_the_reader_cannot_parse_is_one_parse_error_defect() {
    let emulator = emulator();
    // Balanced flow collections, nested deeper than the reader descends.
    let deeply_nested = format!("source: {}{}", "[".repeat(40), "]".repeat(40));

    for (case, configuration) in [
        (
            "a mapping entry followed by a sequence entry at the same level",
            "source:\n  http: {}\n- broken\n".to_string(),
        ),
        (
            "an unterminated flow sequence",
            "source:\n  http: {}\nsink: [cloudwatch_logs\n".to_string(),
        ),
        (
            "a key with no value and a stray colon",
            "source: : http\n".to_string(),
        ),
        (
            "an alias, which expands the document it appears in",
            "base: &anchor\n  http: {}\nsource: *anchor\nsink:\n  - cloudwatch_logs: {}\n"
                .to_string(),
        ),
        ("nesting past what the reader reads", deeply_nested),
    ] {
        let (status, response) = validate(&emulator, &configuration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a defect is reported, not refused; got {status} {response:?}"
        );
        assert_eq!(
            defects(case, &response),
            vec![(PARSE_ERROR.to_string(), "Configuration.Body".to_string())],
            "case {case:?}: expected one parse-error defect, got {response:?}"
        );
    }
}

// =============================================================================
// A document that was read, against the rules a pipeline definition follows
// =============================================================================

/// A document the reader read is checked against the rules a pipeline definition
/// follows, and every defect is reported against the field it is about.
///
/// The rows span the rules: the document declares a pipeline at all, its sections form
/// one chain, each section declares exactly one source and exactly one sink, and its
/// processors are ones this emulator runs with options they take. Each case lists the
/// `(Reason, field)` pairs the answer must carry — so a document with two defects
/// reports two.
///
/// A defect in how the document assembles its pipeline is reported against
/// `Configuration.Body`, the document itself; one PINNED TO A FIELD the document
/// declares is reported under the report's own `path` key, carrying that field's full
/// dotted document path — never the field's own bare name, which cannot tell two fields
/// of that name apart — and an absent required property under the report's `name` key,
/// which is the name of the property to add.
#[tokio::test]
async fn every_defect_of_a_document_is_reported_against_its_field() {
    let emulator = emulator();

    for (case, configuration, expected) in [
        (
            "a document that is not a mapping",
            "just some text\n".to_string(),
            vec![(MISSING_FIELD, "name")],
        ),
        (
            "an empty document declaring no pipeline",
            " \n".to_string(),
            vec![(MISSING_FIELD, "name")],
        ),
        (
            "a mapping declaring no pipeline section",
            "version: \"2\"\ntags:\n  owner: team\n".to_string(),
            vec![(UNKNOWN_PROPERTY, "path"), (MISSING_FIELD, "name")],
        ),
        (
            "a top-level pipeline with no source",
            "processor:\n  - parse_json: {}\nsink:\n  - cloudwatch_logs: {}\n".to_string(),
            // The section names no source, so nothing pins a vended-log branch and each
            // of them is measured against its processor as well.
            [vec![(MISSING_FIELD, "name")], unpinned_branches()].concat(),
        ),
        (
            "a top-level pipeline with no sink",
            "source:\n  http: {}\nprocessor:\n  - parse_json: {}\n".to_string(),
            vec![(MISSING_FIELD, "name")],
        ),
        (
            "a named section with neither a source nor a sink",
            "log-pipeline:\n  processor:\n    - parse_json: {}\n".to_string(),
            [
                vec![(MISSING_FIELD, "name"), (MISSING_FIELD, "name")],
                unpinned_branches(),
            ]
            .concat(),
        ),
        (
            "a section naming two sources",
            "source:\n  http: {}\n  s3: {}\nsink:\n  - cloudwatch_logs: {}\n".to_string(),
            vec![(VALIDATION_ERROR, "path")],
        ),
        (
            "a section naming two sinks",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n  - s3: {}\n".to_string(),
            vec![(VALIDATION_ERROR, "path")],
        ),
        (
            "a source that names no plugin at all",
            "source:\nsink:\n  - cloudwatch_logs: {}\n".to_string(),
            vec![(VALIDATION_ERROR, "path")],
        ),
        (
            "a processor this emulator does not run",
            top_level("processor:\n  - grok:\n      match: \"%{COMMONAPACHELOG}\"\n"),
            vec![(UNSUPPORTED_PROCESSOR, "path")],
        ),
        (
            "an option the processor does not take",
            top_level("processor:\n  - csv:\n      delete_source: true\n"),
            vec![(UNSUPPORTED_PROCESSOR, "path")],
        ),
        (
            "an entry naming two processors at once",
            top_level("processor:\n  - csv: {}\n    trim_string: {}\n"),
            vec![(VALIDATION_ERROR, "Configuration.Body")],
        ),
        (
            "a processor section that is not a sequence",
            "source:\n  http: {}\nprocessor:\n  csv: {}\nsink:\n  - cloudwatch_logs: {}\n"
                .to_string(),
            vec![(VALIDATION_ERROR, "Configuration.Body")],
        ),
        (
            "a pipeline sink naming a section the document does not declare",
            "entry:\n  source:\n    http: {}\n  sink:\n    - pipeline:\n        name: missing\n"
                .to_string(),
            vec![(VALIDATION_ERROR, "Configuration.Body")],
        ),
        (
            "sections handing their records round a cycle",
            "a:\n  \
               source:\n    \
                 pipeline:\n      \
                   name: b\n  \
               sink:\n    \
                 - pipeline:\n        \
                     name: b\n\
             b:\n  \
               source:\n    \
                 pipeline:\n      \
                   name: a\n  \
               sink:\n    \
                 - pipeline:\n        \
                     name: a\n"
                .to_string(),
            vec![(VALIDATION_ERROR, "Configuration.Body")],
        ),
        (
            "a section no sink of the entry section hands its records to",
            named_section("entry", "") + &named_section("orphan", ""),
            vec![(VALIDATION_ERROR, "Configuration.Body")],
        ),
        (
            "a section with a processor defect AND no sink",
            "source:\n  http: {}\nprocessor:\n  - grok: {}\n".to_string(),
            vec![(UNSUPPORTED_PROCESSOR, "path"), (MISSING_FIELD, "name")],
        ),
        (
            "a role ARN that is not an ARN at all",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs:\n      \
               sts_role_arn: TelemetryPipelineRole\n"
                .to_string(),
            vec![(INVALID_PATTERN, "path")],
        ),
        (
            "a role ARN whose account id is not twelve digits",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs:\n      \
               sts_role_arn: \"arn:aws:iam::12345:role/Example\"\n"
                .to_string(),
            vec![(INVALID_PATTERN, "path")],
        ),
        (
            "a role ARN field carrying no value",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs:\n      sts_role_arn:\n".to_string(),
            vec![(INVALID_PATTERN, "path")],
        ),
        (
            "a role ARN field carrying a mapping rather than an ARN",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs:\n      sts_role_arn:\n        \
               name: Example\n"
                .to_string(),
            vec![(INVALID_PATTERN, "path")],
        ),
        (
            "an empty role ARN under the source's aws block",
            "source:\n  s3:\n    aws:\n      sts_role_arn: \"\"\nsink:\n  \
               - cloudwatch_logs: {}\n"
                .to_string(),
            vec![(INVALID_PATTERN, "path")],
        ),
        (
            "a bucket_arn, a field the same rule covers, that is not an ARN",
            format!(
                "source:\n  http: {{}}\nsink:\n  - s3:\n      bucket_arn: telemetry-archive\n      \
                 sts_role_arn: \"{ROLE_ARN}\"\n"
            ),
            vec![(INVALID_PATTERN, "path")],
        ),
        (
            "a role ARN that is a well-formed ARN but not an IAM role's",
            format!(
                "source:\n  http: {{}}\nsink:\n  - s3:\n      sts_role_arn: \"{BUCKET_ARN}\"\n"
            ),
            vec![(INVALID_PATTERN, "path")],
        ),
        (
            "a role field spelled the way JSON writes it, carrying no ARN",
            "{\"source\": {\"http\": {}}, \"sink\": [{\"s3\": {\"stsRoleArn\": \"pipeline-role\"}}]}"
                .to_string(),
            vec![(INVALID_PATTERN, "path")],
        ),
        (
            "an ARN defect AND a structural one, each reported in its own right",
            "source:\n  http: {}\n  s3: {}\nsink:\n  - cloudwatch_logs:\n      \
               sts_role_arn: not-an-arn\n"
                .to_string(),
            vec![(INVALID_PATTERN, "path"), (VALIDATION_ERROR, "path")],
        ),
    ] {
        let (status, response) = validate(&emulator, &configuration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a defect is reported, not refused; got {status} {response:?}"
        );
        let mut expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(reason, field): (&str, &str)| (reason.to_string(), field.to_string()))
            .collect();
        expected.sort();
        assert_eq!(
            defects(case, &response),
            expected,
            "case {case:?}: expected those defects for {configuration}, got {response:?}"
        );
    }
}

// =============================================================================
// What a reported defect says
// =============================================================================

/// A defect says enough to fix the document: the pattern defect names the value that
/// broke the rule, where in the document that value is, and the constraint it did not
/// satisfy — and it takes all three from the request, so a second document with a
/// different field, path and value is described by its own three.
#[tokio::test]
async fn a_pattern_defect_names_the_value_its_path_and_the_constraint() {
    let emulator = emulator();

    for (case, configuration, path, value) in [
        (
            "a role ARN in the source's aws block",
            "log-pipeline:\n  source:\n    s3:\n      aws:\n        \
               sts_role_arn: pipeline-role\n  sink:\n    - cloudwatch_logs: {}\n",
            "log-pipeline.source.s3.aws.sts_role_arn",
            "pipeline-role",
        ),
        (
            "a bucket ARN in the second sink entry of a chained section",
            "entry:\n  source:\n    http: {}\n  sink:\n    - pipeline:\n        name: exit\n\
             exit:\n  source:\n    pipeline:\n      name: entry\n  sink:\n    - s3:\n        \
               bucket_arn: telemetry-archive\n",
            "exit.sink[0].s3.bucket_arn",
            "telemetry-archive",
        ),
    ] {
        let (status, response) = validate(&emulator, configuration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        let errors = response
            .get("Errors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let message = errors
            .iter()
            .filter_map(|error| error.get("Message").and_then(Value::as_str))
            .find(|message| message.contains(path))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a defect naming {path}, got {response:?}")
            });
        assert!(
            message.contains(value),
            "case {case:?}: expected the message to carry the value {value}, got {message}"
        );
        assert!(
            message.contains("pattern") && message.contains("^arn:"),
            "case {case:?}: expected the message to carry the constraint, got {message}"
        );
    }
}

/// Every `ValidationError` of `response` whose `FieldMap` is keyed by `key`, as
/// `(Message, the value under that key)` pairs, sorted so a case asserts the SET of
/// defects rather than the order two independent checks ran in.
fn keyed(case: &str, response: &Value, key: &str) -> Vec<(String, String)> {
    let errors = response
        .get("Errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut reported: Vec<(String, String)> = errors
        .iter()
        .filter_map(|error| {
            let value = error.pointer(&format!("/FieldMap/{key}"))?;
            let value = value.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a FieldMap entry carries text, got {error:?}")
            });
            let message = error
                .get("Message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every defect says what is wrong, got {error:?}")
                });
            Some((message.to_string(), value.to_string()))
        })
        .collect();
    reported.sort();
    reported
}

/// An ABSENT required property is reported by the property's own NAME: the `MISSING_FIELD`
/// defect's `FieldMap` carries that name under the report's `name` key, and the message
/// names the same property — so a client reads which field to add out of the entry
/// rather than guessing it out of a path.
///
/// What the schema requires of a DOCUMENT is the pipeline itself, so a document that
/// declares none is reported as the `pipeline` property being absent; what it requires of
/// a section the document does declare is that section's `source` and `sink`.
///
/// The rows differ along what the rule generalises over: how the document fails to
/// declare a pipeline (a scalar, nothing at all, a mapping of other properties, a
/// sequence), which required property of a declared pipeline is missing (its source, its
/// sink, both), how many of them are, and where the document writes that pipeline (under
/// the property the schema requires, at the top level, under another name).
#[tokio::test]
async fn an_absent_required_property_is_reported_by_its_name() {
    let emulator = emulator();

    for (case, configuration, missing) in [
        (
            "a body that parses as a bare scalar",
            "not-a-pipeline-definition\n".to_string(),
            vec!["pipeline"],
        ),
        ("a body that declares nothing at all", " \n".to_string(), vec!["pipeline"]),
        (
            "a mapping of properties none of which is a pipeline",
            "version: \"2\"\nowner: team\n".to_string(),
            vec!["pipeline"],
        ),
        (
            "a body that parses as a sequence",
            "- cloudwatch_logs\n- s3\n".to_string(),
            vec!["pipeline"],
        ),
        (
            "a declared pipeline missing both of its required properties",
            "pipeline:\n  processor:\n    - parse_json: {}\n".to_string(),
            vec!["sink", "source"],
        ),
        (
            "a declared pipeline missing only its sink",
            "pipeline:\n  source:\n    http: {}\n  processor:\n    - parse_json: {}\n".to_string(),
            vec!["sink"],
        ),
        (
            "a pipeline written at the top level, missing only its source",
            "processor:\n  - parse_json: {}\nsink:\n  - cloudwatch_logs: {}\n".to_string(),
            vec!["source"],
        ),
        (
            "a section the document declares under another name, missing its source",
            "log-pipeline:\n  sink:\n    - cloudwatch_logs: {}\n".to_string(),
            vec!["source"],
        ),
        (
            "a pipeline declaring everything the schema requires of it",
            top_level("processor:\n  - parse_json: {}\n"),
            vec![],
        ),
    ] {
        let (status, response) = validate(&emulator, &configuration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a defect is reported, not refused; got {status} {response:?}"
        );
        let mut expected: Vec<(String, String)> = missing
            .into_iter()
            .map(|property: &str| {
                (
                    format!("required property '{property}' not found"),
                    property.to_string(),
                )
            })
            .collect();
        expected.sort();
        assert_eq!(
            keyed(case, &response, "name"),
            expected,
            "case {case:?}: expected those absent properties for {configuration:?}, \
             got {response:?}"
        );
    }
}

/// Every OTHER defect of a property is reported by WHERE the property is written: the
/// `FieldMap` carries that path under the report's `path` key, named relative to the
/// pipeline the defect is in — the document's own pipeline is the document, so a
/// position of its processor sequence is `processor[<i>]`, while a section the document
/// declares under another name prefixes its own.
///
/// The rows differ along what the rule generalises over: which kind of defect is reported
/// (a property the schema does not define, a property carrying a number of items the
/// schema does not allow, a processor position this emulator cannot run), where in the
/// document it sits (at the root, inside the pipeline the schema's property declares,
/// inside a section declared under another name), and which position of the sequence it
/// is.
#[tokio::test]
async fn a_defect_of_a_property_is_reported_by_where_it_is_written() {
    let emulator = emulator();

    for (case, configuration, expected) in [
        (
            "a property the schema does not define beside the document's pipeline",
            "owner: team\npipeline:\n  source:\n    http: {}\n  sink:\n    \
               - cloudwatch_logs: {}\n"
                .to_string(),
            vec!["root"],
        ),
        (
            "a property the schema does not define inside the document's pipeline",
            "pipeline:\n  source:\n    http: {}\n  retention: 30\n  sink:\n    \
               - cloudwatch_logs: {}\n"
                .to_string(),
            vec!["retention"],
        ),
        (
            "a processor position of the document's own pipeline",
            "pipeline:\n  source:\n    http: {}\n  processor:\n    - grok: {}\n  sink:\n    \
               - cloudwatch_logs: {}\n"
                .to_string(),
            vec!["processor[0]"],
        ),
        (
            "the second position of that sequence",
            "pipeline:\n  source:\n    http: {}\n  processor:\n    - parse_json: {}\n    \
               - grok: {}\n  sink:\n    - cloudwatch_logs: {}\n"
                .to_string(),
            vec!["processor[1]"],
        ),
        (
            "two positions of that sequence, each in its own right",
            "pipeline:\n  source:\n    http: {}\n  processor:\n    - grok: {}\n    \
               - geoip: {}\n  sink:\n    - cloudwatch_logs: {}\n"
                .to_string(),
            vec!["processor[0]", "processor[1]"],
        ),
        (
            "a property the schema does not define inside a section declared under \
             another name",
            "log-pipeline:\n  source:\n    http: {}\n  retries: 3\n  sink:\n    \
               - cloudwatch_logs: {}\n"
                .to_string(),
            vec!["log-pipeline.retries"],
        ),
        (
            "a processor position of a section declared under another name",
            "log-pipeline:\n  source:\n    http: {}\n  processor:\n    - grok: {}\n  \
               sink:\n    - cloudwatch_logs: {}\n"
                .to_string(),
            vec!["log-pipeline.processor[0]"],
        ),
        (
            "a property carrying more items than the schema allows, in the document's \
             pipeline",
            "pipeline:\n  source:\n    http: {}\n    s3: {}\n  sink:\n    \
               - cloudwatch_logs: {}\n"
                .to_string(),
            vec!["source"],
        ),
        (
            "the same property defect in a section declared under another name",
            "log-pipeline:\n  source:\n    http: {}\n    s3: {}\n  sink:\n    \
               - cloudwatch_logs: {}\n"
                .to_string(),
            vec!["log-pipeline.source"],
        ),
        (
            "a pipeline whose every property the schema defines",
            top_level("processor:\n  - parse_json: {}\n"),
            vec![],
        ),
    ] {
        let (status, response) = validate(&emulator, &configuration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a defect is reported, not refused; got {status} {response:?}"
        );
        let mut reported: Vec<String> = keyed(case, &response, "path")
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        reported.sort();
        let mut expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected those paths for {configuration:?}, got {response:?}"
        );
    }
}

// =============================================================================
// Nothing is created and nothing is stored
// =============================================================================

/// The call validates a configuration; it does not deploy one. Nothing it is handed
/// becomes a stored pipeline, so a `GetTelemetryPipeline` for the section the document
/// named is still a 404, the store still holds nothing, and a second identical call
/// answers exactly what the first one did.
#[tokio::test]
async fn validating_a_configuration_creates_nothing() {
    let emulator = emulator();
    let configuration = named_section("log-pipeline", "  processor:\n    - parse_json: {}\n");

    let (status, first) = validate(&emulator, &configuration).await;
    assert_eq!(status, 200, "expected 200, got {status} {first:?}");
    assert!(
        first.get("Errors").is_none(),
        "expected no defect, got {first:?}"
    );

    let (status, read) = call(
        &emulator,
        HOME_REGION,
        "/GetTelemetryPipeline",
        &json!({ "PipelineIdentifier": "log-pipeline" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "a validated configuration is not a stored pipeline, got {status} {read:?}"
    );

    let stored = {
        let state = emulator
            .engine
            .state::<ComAmazonawsObservabilityadminObservabilityadminState>();
        let store = state.lock();
        store.pipeline_count(HOME_REGION)
    };
    assert_eq!(
        stored, 0,
        "the store holds no pipeline after a validate call, got {stored}"
    );

    let (status, second) = validate(&emulator, &configuration).await;
    assert_eq!(status, 200, "expected 200, got {status} {second:?}");
    assert_eq!(
        second, first,
        "a validate reads nothing it could have changed, got {second:?} after {first:?}"
    );
}

/// A configuration this operation reports no defect in is one `CreateTelemetryPipeline`
/// accepts, and the pipeline it then stores carries that very body back out of
/// `GetTelemetryPipeline`: the two operations read the same document, so a clean
/// verdict here is not a verdict about some other document.
#[tokio::test]
async fn a_configuration_reported_clean_is_one_a_create_accepts() {
    let emulator = emulator();
    let configuration = top_level("processor:\n  - parse_json: {}\n");

    let (status, verdict) = validate(&emulator, &configuration).await;
    assert_eq!(status, 200, "expected 200, got {status} {verdict:?}");
    assert!(
        verdict.get("Errors").is_none(),
        "expected no defect, got {verdict:?}"
    );

    let (status, created) = call(
        &emulator,
        HOME_REGION,
        "/CreateTelemetryPipeline",
        &json!({
            "Name": "validated-pipeline",
            "Configuration": { "Body": configuration },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a configuration reported clean is accepted, got {status} {created:?}"
    );

    let (status, read) = call(
        &emulator,
        HOME_REGION,
        "/GetTelemetryPipeline",
        &json!({ "PipelineIdentifier": "validated-pipeline" }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {read:?}");
    assert_eq!(
        read.pointer("/Pipeline/Configuration/Body")
            .and_then(Value::as_str),
        Some(configuration.as_str()),
        "the stored pipeline carries the validated document, got {read:?}"
    );
}
