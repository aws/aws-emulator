// SPDX-License-Identifier: CC0-1.0

//! What an action execution reports about the configuration it RAN with, and what a
//! source action reports when the revision it was asked for is not there.
//!
//! The rules under test:
//!
//! * `ActionExecutionInput$resolvedConfiguration` is "configuration data for an action
//!   execution with all variable references replaced with their real values for the
//!   execution", so every `#{namespace.key}` reference the execution carries a value for
//!   is replaced — the pipeline-level variables, the service's own built-ins, AND the
//!   output variables an earlier action of the same execution published. It is the
//!   configuration the engine ran the action with, which is why the object the run
//!   delivered is at the key the member reports.
//! * a source action that cannot obtain the revision it was asked to source fails with
//!   the `FailureType` the model carries for exactly that condition,
//!   `RevisionUnavailable` (`smithy_query query="shape" shape="FailureType"` ->
//!   `JobFailed`, `ConfigurationError`, `PermissionError`, `RevisionOutOfSync`,
//!   `RevisionUnavailable`, `SystemUnavailable`), rather than as work that ran and
//!   failed.
//!
//! The harness comes from `tests/all/execution_engine.rs`, so these pipelines run their
//! real actions through the emulated S3 service and every assertion goes through the
//! router.

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, wait_for_terminal, Emulator,
};

/// The bucket the source object lives in.
const SOURCE_BUCKET: &str = "resolved-source";

/// The source object's key.
const SOURCE_KEY: &str = "source.zip";

/// The bucket the deploy action delivers into.
const DEPLOY_BUCKET: &str = "resolved-deploy";

/// The artifact the source action produces.
const ARTIFACT: &str = "SourceArtifact";

/// The value the pipeline-level variable every declaration here carries resolves to.
const ENV_VALUE: &str = "staging";

/// One action execution of `execution`, by action name.
async fn action_detail(emulator: &Emulator, pipeline: &str, execution: &str, action: &str) -> Value {
    let page = emulator
        .ok(
            "ListActionExecutions",
            &json!({"pipelineName": pipeline, "filter": {"pipelineExecutionId": execution}}),
        )
        .await;
    page["actionExecutionDetails"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|detail| detail["actionName"] == json!(action))
        .cloned()
        .unwrap_or_else(|| panic!("the run of {action} is reported, got {page}"))
}

/// RULE: the resolved configuration an action execution reports is the one the engine
/// ran it with — every reference the execution carries a value for replaced, in whatever
/// namespace, and the declared configuration still reported as declared.
#[tokio::test]
async fn resolved_configuration_replaces_every_reference_the_execution_carries() {
    let emulator = Emulator::new();
    let pipeline = "resolved-pipe";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    let version = emulator
        .put_object(SOURCE_BUCKET, SOURCE_KEY, b"resolved bytes")
        .await
        .expect("a versioned bucket reports the version it wrote");

    // (case, the ObjectKey declared, the ObjectKey the execution resolves it to). The
    // rows differ along the FORM of the reference: none at all, one on its own, one
    // inside surrounding text, two from different namespaces, and the service's own
    // built-in.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "no reference at all",
            "plain/out.zip".to_string(),
            "plain/out.zip".to_string(),
        ),
        (
            "an earlier action's output variable on its own",
            "#{SourceVariables.VersionId}".to_string(),
            version.clone(),
        ),
        (
            "an output variable inside surrounding text",
            "v/#{SourceVariables.VersionId}/out.zip".to_string(),
            format!("v/{version}/out.zip"),
        ),
        (
            "a pipeline variable and an output variable",
            "#{variables.Env}/#{SourceVariables.ObjectKey}".to_string(),
            format!("{ENV_VALUE}/{SOURCE_KEY}"),
        ),
    ];

    for (index, (case, declared, expected)) in rows.into_iter().enumerate() {
        let name = format!("{pipeline}-{index}");
        emulator.bucket(&artifact_bucket(&name)).await;
        let mut body = declaration(
            &name,
            json!([
                source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, &declared, false),
            ]),
            None,
        );
        body["pipeline"]["variables"] = json!([{"name": "Env", "defaultValue": ENV_VALUE}]);
        emulator.ok("CreatePipeline", &body).await;
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": name}))
            .await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .expect("a start reports the execution it began")
            .to_string();
        let answer = wait_for_terminal(&emulator, &name, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"], json!("Succeeded"),
            "case {case:?}: the execution runs with the resolved configuration, \
             got {answer}"
        );

        let detail = action_detail(&emulator, &name, &execution, "Dep").await;
        assert_eq!(
            detail["input"]["resolvedConfiguration"]["ObjectKey"], json!(expected),
            "case {case:?}: the resolved configuration holds the substituted key, \
             got {detail}"
        );
        assert_eq!(
            detail["input"]["configuration"]["ObjectKey"], json!(declared),
            "case {case:?}: the declared configuration is reported as declared, \
             got {detail}"
        );
        // And the run really used it: the object is at the resolved key, not the
        // template.
        let (status, body) = emulator.get_object(DEPLOY_BUCKET, &expected).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delivered object is at the resolved key {expected:?}, \
             got {status}"
        );
        assert_eq!(
            body, b"resolved bytes",
            "case {case:?}: the delivered object carries the source bytes"
        );
    }
}

/// RULE: the built-in execution-id reference resolves to the id of the execution doing
/// the resolving, so two runs of one pipeline resolve the same declaration differently.
#[tokio::test]
async fn the_built_in_execution_id_resolves_per_execution() {
    let emulator = Emulator::new();
    let pipeline = "resolved-built-in";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    emulator
        .put_object(SOURCE_BUCKET, SOURCE_KEY, b"per-run bytes")
        .await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                    deploy_stage(
                        "Deploy",
                        ARTIFACT,
                        DEPLOY_BUCKET,
                        "#{codepipeline.PipelineExecutionId}/out.zip",
                        false,
                    ),
                ]),
                None,
            ),
        )
        .await;

    for case in ["the first run", "the second run"] {
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .expect("a start reports the execution it began")
            .to_string();
        let answer = wait_for_terminal(&emulator, pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"], json!("Succeeded"),
            "case {case:?}: the execution runs, got {answer}"
        );
        let detail = action_detail(&emulator, pipeline, &execution, "Dep").await;
        assert_eq!(
            detail["input"]["resolvedConfiguration"]["ObjectKey"],
            json!(format!("{execution}/out.zip")),
            "case {case:?}: the built-in resolves to this execution's own id, \
             got {detail}"
        );
        let (status, _) = emulator
            .get_object(DEPLOY_BUCKET, &format!("{execution}/out.zip"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the object is delivered under this execution's own id, \
             got {status}"
        );
    }
}

/// RULE: the OBJECT an S3 source action names is the revision it sources, so an object
/// (or an object version) the bucket does not hold is a revision that is not available —
/// while a bucket the service does not hold at all is a source the declaration cannot
/// read from, which is a configuration error. Either way nothing is delivered
/// downstream.
#[tokio::test]
async fn a_revision_that_cannot_be_obtained_fails_with_revision_unavailable() {
    // (case, the source bucket exists, the object exists, the object version pinned,
    //  the `FailureType` the action reports)
    let rows: Vec<(&str, bool, bool, Option<&str>, &str)> = vec![
        ("no such object", true, false, None, "RevisionUnavailable"),
        (
            "an object version the bucket does not hold",
            true,
            true,
            Some("00000000000000000000000000000001"),
            "RevisionUnavailable",
        ),
        (
            "no such bucket",
            false,
            false,
            None,
            "ConfigurationError",
        ),
    ];

    for (index, (case, make_bucket, write_source, pinned, expected_code)) in
        rows.into_iter().enumerate()
    {
        let emulator = Emulator::new();
        let pipeline = format!("revision-gone-{index}");
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket(DEPLOY_BUCKET).await;
        if make_bucket {
            emulator.bucket(SOURCE_BUCKET).await;
        }
        if write_source {
            emulator
                .put_object(SOURCE_BUCKET, SOURCE_KEY, b"the wrong revision")
                .await;
        }
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                        deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, "gone.zip", false),
                    ]),
                    None,
                ),
            )
            .await;
        let mut start = json!({"name": pipeline});
        if let Some(version) = pinned {
            start["sourceRevisions"] = json!([{
                "actionName": "Src",
                "revisionType": "S3_OBJECT_VERSION_ID",
                "revisionValue": version,
            }]);
        }
        let started = emulator.ok("StartPipelineExecution", &start).await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .expect("a start reports the execution it began")
            .to_string();
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"], json!("Failed"),
            "case {case:?}: a revision that cannot be obtained fails the execution, \
             got {answer}"
        );

        let detail = action_detail(&emulator, &pipeline, &execution, "Src").await;
        let error = detail["output"]["executionResult"]["errorDetails"].clone();
        assert_eq!(
            error["code"], json!(expected_code),
            "case {case:?}: unexpected failure type, got {error}"
        );
        assert!(
            error["message"]
                .as_str()
                .is_some_and(|message| message.contains(SOURCE_KEY)),
            "case {case:?}: the reason names the object that could not be read, \
             got {error}"
        );
        let (status, _) = emulator.get_object(DEPLOY_BUCKET, "gone.zip").await;
        assert_eq!(
            status, 404,
            "case {case:?}: nothing is delivered from a revision that was never read, \
             got {status}"
        );
    }
}

/// The internal representation of the client error object an answer arrives in, which is
/// no fact about a pipeline: the Rust type and field names of the error, the raw
/// response document it carries and the per-call ids inside that document.
const CLIENT_INTERNALS: [&str; 7] = [
    "ServiceError",
    "ErrorMetadata",
    "SdkBody",
    "StatusCode",
    "RequestId",
    "HostId",
    "Response {",
];

/// The modelled length of `ErrorDetails$message` -> `Message`, `@length(min: 1, max:
/// 5000)` (`smithy_query query="shape" shape="ErrorDetails"`).
const MAX_ERROR_MESSAGE: usize = 5_000;

/// The modelled length of `ActionExecution$summary` -> `ExecutionSummary`,
/// `@length(min: 1, max: 2048)` (`smithy_query query="shape" shape="ActionExecution"`).
const MAX_ACTION_SUMMARY: usize = 2_048;

/// The `summary` the state reports for one action's latest execution.
async fn action_summary(emulator: &Emulator, pipeline: &str, action: &str) -> String {
    let state = emulator
        .ok("GetPipelineState", &json!({"name": pipeline}))
        .await;
    state["stageStates"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|stage| stage["actionStates"].as_array().into_iter().flatten())
        .find(|state| state["actionName"] == json!(action))
        .and_then(|state| state["latestExecution"]["summary"].as_str())
        .unwrap_or_else(|| panic!("the state reports the run of {action}, got {state}"))
        .to_string()
}

/// RULE: a failure an action reports relays the answering service's OWN error — the
/// code and the message that service published — inside this service's sentence about
/// what could not be done, and both members carrying it stay within the length they are
/// modelled with. The internal representation of the client error object the answer
/// arrived in is not part of either.
///
/// The rows differ along the error the source read is answered with, each a different
/// fact about the read: the key, the version, and the bucket.
#[tokio::test]
async fn a_failure_relays_the_answering_services_error_and_none_of_the_client_object() {
    // (case, the source bucket exists, the object exists, the object version pinned,
    //  the error code the S3 read is answered with)
    let rows: Vec<(&str, bool, bool, Option<&str>, &str)> = vec![
        ("no such object", true, false, None, "NoSuchKey"),
        (
            "an object version the bucket does not hold",
            true,
            true,
            Some("00000000000000000000000000000001"),
            "NoSuchVersion",
        ),
        ("no such bucket", false, false, None, "NoSuchBucket"),
    ];

    for (index, (case, make_bucket, write_source, pinned, answered_code)) in
        rows.into_iter().enumerate()
    {
        let emulator = Emulator::new();
        let pipeline = format!("relayed-{index}");
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket(DEPLOY_BUCKET).await;
        if make_bucket {
            emulator.bucket(SOURCE_BUCKET).await;
        }
        if write_source {
            emulator
                .put_object(SOURCE_BUCKET, SOURCE_KEY, b"the wrong revision")
                .await;
        }
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                        deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, "gone.zip", false),
                    ]),
                    None,
                ),
            )
            .await;
        let mut start = json!({"name": pipeline});
        if let Some(version) = pinned {
            start["sourceRevisions"] = json!([{
                "actionName": "Src",
                "revisionType": "S3_OBJECT_VERSION_ID",
                "revisionValue": version,
            }]);
        }
        let started = emulator.ok("StartPipelineExecution", &start).await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .expect("a start reports the execution it began")
            .to_string();
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"], json!("Failed"),
            "case {case:?}: a revision that cannot be obtained fails the execution, \
             got {answer}"
        );

        let detail = action_detail(&emulator, &pipeline, &execution, "Src").await;
        let message = detail["output"]["executionResult"]["errorDetails"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the failure states a message, got {detail}"))
            .to_string();
        let summary = action_summary(&emulator, &pipeline, "Src").await;
        for (member, text, limit) in [
            ("errorDetails.message", &message, MAX_ERROR_MESSAGE),
            ("summary", &summary, MAX_ACTION_SUMMARY),
        ] {
            // The answering service's own error, relayed: its code, and the object the
            // read was of.
            assert!(
                text.contains(answered_code) && text.contains(SOURCE_KEY),
                "case {case:?}: {member} relays the {answered_code} answer about \
                 {SOURCE_KEY}, got {text:?}"
            );
            for internal in CLIENT_INTERNALS {
                assert!(
                    !text.contains(internal),
                    "case {case:?}: {member} carries the client object's {internal:?}, \
                     got {text:?}"
                );
            }
            let measured = text.chars().count();
            assert!(
                (1..=limit).contains(&measured),
                "case {case:?}: {member} is modelled 1..={limit} characters, got \
                 {measured}"
            );
        }
    }
}
