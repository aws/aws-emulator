// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for a run's output destination and its parameter source.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The HTTP harness is the one
//! `private_workflow_runs` owns.
//!
//! The rules under test:
//!
//! * `outputUriPrefixMode` decides which prefixes the service adds beneath the run's
//!   `outputUri`, and the resolved mode is reported for every run: the mode that adds a
//!   run-specific prefix puts the run's own id beneath `outputUri`, the mode that adds
//!   none leaves `outputUri` itself as the destination, and a run that named no mode
//!   resolves to the one the member's documentation names as the service's default;
//! * `requesterPaysEnabled` is reported as the run was started with it, and defaults to
//!   `false`;
//! * a run's parameters come from exactly one source — the inline document or the file
//!   URI — so a request naming both is refused and a read reports whichever one the run
//!   carries;
//! * every one of these is part of the stored run, so a run duplicated from another
//!   inherits it unless the duplicate names its own.

use serde_json::{json, Map, Value};

use crate::private_workflow_runs::{
    assert_absent, created_id, emulator, error_code, get_run, start_run, workflow_for_runs,
};

/// A role ARN satisfying the modelled `RunRoleArn` pattern (`^arn:.+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmicsWorkflowRole";

/// Every value the modelled `RunOutputUriPrefixMode` enum admits.
/// Source: `smithy_query(query="shape",shape="RunOutputUriPrefixMode")` ->
/// SERVICE_DEFINED | NONE_WITH_OVERWRITE.
const OUTPUT_URI_PREFIX_MODES: &[&str] = &["SERVICE_DEFINED", "NONE_WITH_OVERWRITE"];

/// The mode in force when `StartRun` names none.
/// Source: the `outputUriPrefixMode` member of `StartRunRequest` ("If you don't specify a
/// value, the service uses SERVICE_DEFINED, which adds a run-specific prefix") and of
/// `GetRunResponse` ("The service returns SERVICE_DEFINED if the request did not specify
/// a value").
const DEFAULT_OUTPUT_URI_PREFIX_MODE: &str = "SERVICE_DEFINED";

/// An S3 URI satisfying the modelled `RunParametersUri` pattern
/// (`^s3://[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]/.+$`).
const PARAMETERS_URI: &str = "s3://omics-emulator-bucket/params/run-parameters.json";

/// The destination a run under `output_uri` reports when the mode in force is `mode`.
///
/// `SERVICE_DEFINED` "adds a run-specific prefix", so the run's own id lands beneath the
/// caller's `outputUri`; `NONE_WITH_OVERWRITE` "adds no prefix and overwrites existing
/// objects at the run's output keys", so the destination is the submitted `outputUri`.
fn expected_destination(output_uri: &str, id: &str, mode: &str) -> String {
    if mode == "NONE_WITH_OVERWRITE" {
        return output_uri.to_string();
    }
    format!("{}/{id}", output_uri.trim_end_matches('/'))
}

/// A `StartRun` body carrying the required members plus the row's own.
fn run_body(workflow_id: &str, request_id: &str, output_uri: &str, extra: &Value) -> Value {
    let mut body = Map::new();
    body.insert("workflowId".to_string(), json!(workflow_id));
    body.insert("roleArn".to_string(), json!(ROLE_ARN));
    body.insert("outputUri".to_string(), json!(output_uri));
    body.insert("requestId".to_string(), json!(request_id));
    for (member, value) in extra.as_object().expect("extra members are an object") {
        body.insert(member.clone(), value.clone());
    }
    Value::Object(body)
}

/// The member of `response`, as a string, for an assertion message.
fn member<'a>(response: &'a Value, name: &str) -> Option<&'a str> {
    response.get(name).and_then(Value::as_str)
}

#[tokio::test]
async fn the_output_uri_prefix_mode_decides_the_destination_and_is_reported() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "prefix-mode-workflow").await;

    // (case, outputUri, the mode the request names, the mode in force)
    let mut rows: Vec<(String, &str, Option<&str>, &str)> = vec![
        (
            "no mode named".to_string(),
            "s3://omics-emulator-bucket/runs",
            None,
            DEFAULT_OUTPUT_URI_PREFIX_MODE,
        ),
        (
            "no mode named, trailing slash".to_string(),
            "s3://omics-emulator-bucket/runs/",
            None,
            DEFAULT_OUTPUT_URI_PREFIX_MODE,
        ),
    ];
    // Every value the enum admits, each against an `outputUri` with and without a
    // trailing separator, so the prefix the mode asks for is the only difference.
    for mode in OUTPUT_URI_PREFIX_MODES {
        for (shape, output_uri) in [
            ("bare", "s3://omics-emulator-bucket/runs"),
            ("trailing slash", "s3://omics-emulator-bucket/runs/"),
            ("nested key", "s3://omics-emulator-bucket/team/runs/2024"),
        ] {
            rows.push((format!("{mode} with a {shape} outputUri"), output_uri, Some(mode), mode));
        }
    }

    for (index, (case, output_uri, named, in_force)) in rows.iter().enumerate() {
        let mut extra = Map::new();
        if let Some(named) = named {
            extra.insert("outputUriPrefixMode".to_string(), json!(named));
        }
        let body = run_body(
            &workflow_id,
            &format!("prefix-mode-{index}"),
            output_uri,
            &Value::Object(extra),
        );
        let (status, response) = start_run(&router, &body).await;
        let id = created_id(case, status, &response);
        let expected = expected_destination(output_uri, &id, in_force);
        assert_eq!(
            member(&response, "runOutputUri"),
            Some(expected.as_str()),
            "case {case:?}: StartRun must report the destination the mode asks for \
             ({expected}), got {:?}",
            member(&response, "runOutputUri")
        );

        let (status, read) = get_run(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {read:?}");
        assert_eq!(
            member(&read, "outputUriPrefixMode"),
            Some(*in_force),
            "case {case:?}: expected the mode in force {in_force}, got {:?}",
            member(&read, "outputUriPrefixMode")
        );
        assert_eq!(
            member(&read, "outputUri"),
            Some(*output_uri),
            "case {case:?}: outputUri must be the one submitted, got {:?}",
            member(&read, "outputUri")
        );
        assert_eq!(
            member(&read, "runOutputUri"),
            Some(expected.as_str()),
            "case {case:?}: a read must report the same destination ({expected}), got {:?}",
            member(&read, "runOutputUri")
        );
    }
}

#[tokio::test]
async fn requester_pays_is_reported_as_the_run_was_started() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "requester-pays-workflow").await;

    // (case, the value the request names, the value in force). The member's own
    // documentation: "The default is false."
    let rows: Vec<(&str, Option<bool>, bool)> = vec![
        ("not named", None, false),
        ("enabled", Some(true), true),
        ("disabled", Some(false), false),
    ];

    for (index, (case, named, in_force)) in rows.iter().enumerate() {
        let mut extra = Map::new();
        if let Some(named) = named {
            extra.insert("requesterPaysEnabled".to_string(), json!(named));
        }
        let body = run_body(
            &workflow_id,
            &format!("requester-pays-{index}"),
            "s3://omics-emulator-bucket/runs",
            &Value::Object(extra),
        );
        let (status, response) = start_run(&router, &body).await;
        let id = created_id(case, status, &response);

        let (status, read) = get_run(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {read:?}");
        assert_eq!(
            read.get("requesterPaysEnabled").and_then(Value::as_bool),
            Some(*in_force),
            "case {case:?}: expected requesterPaysEnabled {in_force}, got {:?}",
            read.get("requesterPaysEnabled")
        );
    }
}

#[tokio::test]
async fn a_run_carries_one_parameter_source_and_reads_it_back() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "parameter-source-workflow").await;

    // (case, parameters, parametersUri, expected status)
    let rows: Vec<(&str, Value, Option<&str>, u16)> = vec![
        ("neither source", Value::Null, None, 201),
        (
            "an inline document",
            json!({ "sample": "NA12878" }),
            None,
            201,
        ),
        ("an empty inline document", json!({}), None, 201),
        ("a parameters file", Value::Null, Some(PARAMETERS_URI), 201),
        (
            "a parameters file with a nested key",
            Value::Null,
            Some("s3://omics-emulator-bucket/a/b/c/params.json"),
            201,
        ),
        (
            "both sources",
            json!({ "sample": "NA12878" }),
            Some(PARAMETERS_URI),
            400,
        ),
        (
            "both sources, the document empty",
            json!({}),
            Some(PARAMETERS_URI),
            400,
        ),
    ];

    for (index, (case, parameters, parameters_uri, expected)) in rows.iter().enumerate() {
        let mut extra = Map::new();
        if !parameters.is_null() {
            extra.insert("parameters".to_string(), parameters.clone());
        }
        if let Some(parameters_uri) = parameters_uri {
            extra.insert("parametersUri".to_string(), json!(parameters_uri));
        }
        let body = run_body(
            &workflow_id,
            &format!("parameter-source-{index}"),
            "s3://omics-emulator-bucket/runs",
            &Value::Object(extra),
        );
        let (status, response) = start_run(&router, &body).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected {expected}, got {response:?}"
        );
        if *expected == 400 {
            // The rule the `parametersUri` member states: "Specify either parameters or
            // parametersUri, not both."
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response:?}"
            );
            assert_eq!(
                member(&response, "message"),
                Some("Specify either parameters or parametersUri, not both."),
                "case {case:?}: expected the rule's own sentence, got {response:?}"
            );
            continue;
        }

        let id = created_id(case, status, &response);
        let (status, read) = get_run(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {read:?}");
        match parameters_uri {
            // "For a run that you started with parametersUri, the service returns the
            // location of the run parameters in this field instead of parameters."
            Some(parameters_uri) => {
                assert_eq!(
                    member(&read, "parametersUri"),
                    Some(*parameters_uri),
                    "case {case:?}: expected the submitted parameters file, got {read:?}"
                );
                assert_absent(case, &["parameters"], &read);
            }
            None => {
                assert_absent(case, &["parametersUri"], &read);
                assert_eq!(
                    read.get("parameters"),
                    parameters.as_object().map(|_| parameters),
                    "case {case:?}: expected the inline document the run was started \
                     with, got {read:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_duplicated_run_inherits_its_destination_and_parameter_source() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "duplicate-source-workflow").await;
    let source_output_uri = "s3://omics-emulator-bucket/source";
    let body = run_body(
        &workflow_id,
        "duplicate-source",
        source_output_uri,
        &json!({
            "outputUriPrefixMode": "NONE_WITH_OVERWRITE",
            "requesterPaysEnabled": true,
            "parametersUri": PARAMETERS_URI,
        }),
    );
    let (status, response) = start_run(&router, &body).await;
    let source_id = created_id("duplicate source", status, &response);

    // A duplicate that names none of them takes the source run's values, and the
    // inherited mode decides its own destination — under its own outputUri.
    let inherit_output_uri = "s3://omics-emulator-bucket/inherited/";
    let (status, response) = start_run(
        &router,
        &json!({
            "runId": source_id,
            "roleArn": ROLE_ARN,
            "outputUri": inherit_output_uri,
            "requestId": "duplicate-inherits",
        }),
    )
    .await;
    let inherited_id = created_id("duplicate inherits", status, &response);
    let (status, read) = get_run(&router, &inherited_id).await;
    assert_eq!(status, 200, "inherited duplicate: expected 200, got {read:?}");
    assert_eq!(
        member(&read, "outputUriPrefixMode"),
        Some("NONE_WITH_OVERWRITE"),
        "inherited duplicate: expected the source run's mode, got {read:?}"
    );
    assert_eq!(
        member(&read, "runOutputUri"),
        Some(inherit_output_uri),
        "inherited duplicate: the inherited mode adds no prefix, so the destination is \
         this run's own outputUri, got {read:?}"
    );
    assert_eq!(
        read.get("requesterPaysEnabled").and_then(Value::as_bool),
        Some(true),
        "inherited duplicate: expected the source run's requesterPaysEnabled, got {read:?}"
    );
    assert_eq!(
        member(&read, "parametersUri"),
        Some(PARAMETERS_URI),
        "inherited duplicate: expected the source run's parameters file, got {read:?}"
    );

    // A duplicate that names its own values overrides each of them, and an inline
    // document REPLACES the inherited file: one run carries one parameter source.
    let override_output_uri = "s3://omics-emulator-bucket/overridden";
    let (status, response) = start_run(
        &router,
        &json!({
            "runId": source_id,
            "roleArn": ROLE_ARN,
            "outputUri": override_output_uri,
            "requestId": "duplicate-overrides",
            "outputUriPrefixMode": "SERVICE_DEFINED",
            "requesterPaysEnabled": false,
            "parameters": { "sample": "NA12878" },
        }),
    )
    .await;
    let overridden_id = created_id("duplicate overrides", status, &response);
    let (status, read) = get_run(&router, &overridden_id).await;
    assert_eq!(
        status, 200,
        "overriding duplicate: expected 200, got {read:?}"
    );
    assert_eq!(
        member(&read, "outputUriPrefixMode"),
        Some("SERVICE_DEFINED"),
        "overriding duplicate: expected this request's mode, got {read:?}"
    );
    assert_eq!(
        member(&read, "runOutputUri"),
        Some(format!("{override_output_uri}/{overridden_id}").as_str()),
        "overriding duplicate: SERVICE_DEFINED adds this run's own prefix, got {read:?}"
    );
    assert_eq!(
        read.get("requesterPaysEnabled").and_then(Value::as_bool),
        Some(false),
        "overriding duplicate: expected this request's requesterPaysEnabled, got {read:?}"
    );
    assert_eq!(
        read.get("parameters"),
        Some(&json!({ "sample": "NA12878" })),
        "overriding duplicate: expected this request's inline document, got {read:?}"
    );
    assert_absent("overriding duplicate", &["parametersUri"], &read);

    // The source run is untouched by either duplicate.
    let (status, read) = get_run(&router, &source_id).await;
    assert_eq!(status, 200, "source run: expected 200, got {read:?}");
    assert_eq!(
        member(&read, "runOutputUri"),
        Some(source_output_uri),
        "source run: a duplicate must not move the destination it reports, got {read:?}"
    );
    assert_eq!(
        member(&read, "parametersUri"),
        Some(PARAMETERS_URI),
        "source run: a duplicate must not change its parameter source, got {read:?}"
    );
}

#[tokio::test]
async fn a_run_started_with_a_parameters_file_reports_it_on_every_read() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "parameters-file-workflow").await;
    let body = run_body(
        &workflow_id,
        "parameters-file",
        "s3://omics-emulator-bucket/runs",
        &json!({ "parametersUri": PARAMETERS_URI }),
    );
    let (status, response) = start_run(&router, &body).await;
    let id = created_id("parameters file", status, &response);

    // A read reports the URI the run was started with and never moves it: this emulator
    // stores no copy of the file, so there is no second location to report.
    let (status, first) = get_run(&router, &id).await;
    assert_eq!(status, 200, "first read: expected 200, got {first:?}");
    let (status, second) = get_run(&router, &id).await;
    assert_eq!(status, 200, "second read: expected 200, got {second:?}");
    assert_eq!(
        member(&first, "parametersUri"),
        member(&second, "parametersUri"),
        "two reads must report the same parameters file, got {first:?} then {second:?}"
    );
    assert_eq!(
        member(&second, "parametersUri"),
        Some(PARAMETERS_URI),
        "expected the submitted parameters file, got {second:?}"
    );
}
