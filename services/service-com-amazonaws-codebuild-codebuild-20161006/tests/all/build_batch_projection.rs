// SPDX-License-Identifier: CC0-1.0

//! Cover for what a BATCH BUILD reports once it has run, driven over the wire the router
//! serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! Three rules:
//!
//! 1. A batch build runs under batch build options, and EITHER source states them: the
//!    project's own `buildBatchConfig` or the `buildBatchConfigOverride` the start call
//!    carries. The started batch reports the configuration it RUNS WITH — the stated
//!    members as stated, and CodeBuild's resolved defaults for the ones left unstated
//!    (`state::DEFAULT_BATCH_TIMEOUT_MINUTES`,
//!    `state::DEFAULT_MAXIMUM_BUILDS_ALLOWED`) — the same way it reports its cache,
//!    source and environment.
//! 2. A batch build fans out into the source download CodeBuild runs for the batch first
//!    (`state::SOURCE_DOWNLOAD_IDENTIFIER`) and one build per identifier its batchspec
//!    declares; it then walks its own phases to a terminal outcome decided by those
//!    builds — `SUBMITTED`, `DOWNLOAD_BATCHSPEC`, `IN_PROGRESS`, the
//!    `COMBINE_ARTIFACTS` phase the model documents as the batch's own finishing work
//!    ("build output artifacts are being combined and uploaded to the output location"),
//!    and the terminal marker phase — and each of its groups reports the build it named
//!    with that build's status and the artifacts it RESOLVED.
//! 3. A batch reports the artifacts DESTINATION it runs with: its type in the token form
//!    the service states (`state::wire_artifacts_type`) and its location, stated as the
//!    empty string for a destination that names none.
//!
//! The table spans the class rather than one recorded batch: the batchspec's build type
//! and the count of identifiers it declares, whether the groups depend on each other,
//! where the batch options come from, and the artifacts destination (each of the model's
//! `ArtifactsType` values — `smithy_query(query="shape", shape="ArtifactsType")`), which
//! also decides the outcome the builds settle on: a build bound for a bucket uploads
//! nothing from this build environment, so its batch ends FAILED, while one with no
//! destination to upload to ends SUCCEEDED.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{call, create_project, emulator, merged};

/// How long a batch build's walk to a terminal status is given here.
///
/// Generous by design: the loop below waits for the CONDITION — the batch reporting
/// itself complete — rather than for a span to pass, so a loaded or slow host only takes
/// more turns through the loop. Nothing here asserts on elapsed time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long the poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A `CreateProject` source whose inline buildspec is `buildspec`.
fn source_with(buildspec: &str) -> Value {
    json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}})
}

/// A buildspec whose `batch` section declares `build-list` entries for `identifiers`, and
/// whose own `build` phase runs a command every build of the batch can run.
fn build_list_spec(identifiers: &[&str]) -> String {
    let mut spec = String::from("version: 0.2\nbatch:\n  build-list:\n");
    for identifier in identifiers {
        spec.push_str(&format!("    - identifier: {identifier}\n"));
    }
    spec.push_str("phases:\n  build:\n    commands:\n      - echo building\n");
    spec
}

/// A buildspec whose `batch` section declares a `build-graph` of `base` and a `leaf` that
/// waits for it.
fn build_graph_spec() -> String {
    "version: 0.2\n\
     batch:\n  \
       build-graph:\n    \
         - identifier: base\n    \
         - identifier: leaf\n      \
           depends-on:\n        \
             - base\n\
     phases:\n  \
       build:\n    \
         commands:\n      \
           - echo building\n"
        .to_string()
}

/// Start a batch build, asserting the call succeeded, and hand back the batch.
async fn start_build_batch(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, body) = call(router, "StartBuildBatch", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartBuildBatch {request} expected 200, got {status} with {body}"
    );
    body["buildBatch"].clone()
}

/// Read the batch `id` until it reports itself COMPLETE, and hand that record back.
///
/// A poll loop rather than a wait: the batch's walk is paced by the service, so the test
/// looks again until the condition holds or the generous deadline above passes.
async fn settled_batch(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = call(router, "BatchGetBuildBatches", json!({"ids": [id]})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetBuildBatches {id} expected 200, got {status} with {body}"
        );
        let batch = body["buildBatches"][0].clone();
        if batch["complete"] == json!(true) {
            return batch;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the batch build never reached a terminal status, last read \
             {batch}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `(phaseType, phaseStatus)` pairs a batch build reports, in order.
fn phase_pairs(batch: &Value) -> Vec<(String, Option<String>)> {
    batch["phases"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|phase| {
            (
                phase["phaseType"].as_str().unwrap_or_default().to_string(),
                phase["phaseStatus"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// The identifiers a batch's build groups report, in order.
fn group_identifiers(batch: &Value) -> Vec<String> {
    batch["buildGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|group| group["identifier"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the table: a batch build to start and everything its settled read must say.
struct Row {
    /// What the row is, for every assertion message.
    case: &'static str,
    /// The buildspec the project carries.
    buildspec: String,
    /// The project's `artifacts`.
    artifacts: Value,
    /// The project's `buildBatchConfig`, when it carries one.
    project_config: Option<Value>,
    /// The start call's `buildBatchConfigOverride`, when it carries one.
    config_override: Option<Value>,
    /// The identifiers the batchspec DECLARES, in order.
    declared: Vec<&'static str>,
    /// The status the batch must settle on.
    outcome: &'static str,
    /// The `timeoutInMins` the batch must run under.
    timeout_in_mins: i32,
    /// The `restrictions.maximumBuildsAllowed` the batch must run under.
    maximum_builds: i32,
    /// The artifacts `location` the batch must report.
    location: &'static str,
}

/// The rows: each one differs from the others along the batchspec, the artifacts
/// destination, where the batch options come from, and what the builds settle on.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a build-list of one identifier, no artifacts, options on the project",
            buildspec: build_list_spec(&["build1"]),
            artifacts: json!({"type": "NO_ARTIFACTS"}),
            project_config: Some(json!({"timeoutInMins": 60})),
            config_override: None,
            declared: vec!["build1"],
            outcome: "SUCCEEDED",
            timeout_in_mins: 60,
            maximum_builds: state::DEFAULT_MAXIMUM_BUILDS_ALLOWED,
            location: "",
        },
        Row {
            case: "a build-list of three identifiers, options stated only by the request",
            buildspec: build_list_spec(&["one", "two", "three"]),
            artifacts: json!({"type": "NO_ARTIFACTS"}),
            project_config: None,
            config_override: Some(
                json!({"serviceRole": "arn:aws:iam::000000000000:role/service-role/batch"}),
            ),
            declared: vec!["one", "two", "three"],
            outcome: "SUCCEEDED",
            timeout_in_mins: state::DEFAULT_BATCH_TIMEOUT_MINUTES,
            maximum_builds: state::DEFAULT_MAXIMUM_BUILDS_ALLOWED,
            location: "",
        },
        Row {
            case: "a build-graph whose leaf waits for its base, artifacts CodePipeline \
                   manages",
            buildspec: build_graph_spec(),
            artifacts: json!({"type": "CODEPIPELINE"}),
            project_config: Some(json!({"restrictions": {"maximumBuildsAllowed": 7}})),
            config_override: None,
            declared: vec!["base", "leaf"],
            outcome: "SUCCEEDED",
            timeout_in_mins: state::DEFAULT_BATCH_TIMEOUT_MINUTES,
            maximum_builds: 7,
            location: "",
        },
        Row {
            case: "a build-list bound for a bucket, which its builds upload nothing to",
            buildspec: build_list_spec(&["uploaded"]),
            artifacts: json!({"type": "S3", "location": "artifacts-bucket/out"}),
            project_config: Some(json!({"timeoutInMins": 2160})),
            config_override: Some(json!({"timeoutInMins": 5})),
            declared: vec!["uploaded"],
            outcome: "FAILED",
            timeout_in_mins: 5,
            maximum_builds: state::DEFAULT_MAXIMUM_BUILDS_ALLOWED,
            location: "artifacts-bucket/out",
        },
    ]
}

/// RULES 1-3 together, over the whole table: every row starts one batch build and reads
/// it back once it is complete.
#[tokio::test]
async fn a_settled_batch_build_reports_its_groups_phases_and_destination() {
    for (index, row) in rows().into_iter().enumerate() {
        let router = emulator();
        let project = format!("batch-projection-{index}");
        let mut members = merged(
            source_with(&row.buildspec),
            json!({"artifacts": row.artifacts.clone()}),
        );
        if let Some(config) = row.project_config.clone() {
            members = merged(members, json!({"buildBatchConfig": config}));
        }
        create_project(&router, &project, members).await;

        let mut request = json!({"projectName": project.clone()});
        if let (Value::Object(target), Some(config)) = (&mut request, row.config_override.clone()) {
            target.insert("buildBatchConfigOverride".to_string(), config);
        }
        let started = start_build_batch(&router, row.case, request).await;
        let id = started["id"].as_str().unwrap_or_default().to_string();
        let batch_arn = started["arn"].as_str().unwrap_or_default().to_string();

        // RULE 1: the batch runs with the configuration either source stated, with the
        // resolved defaults filled in — reported by the start call and by the read alike.
        let case = row.case;
        assert_eq!(
            started["buildBatchConfig"]["timeoutInMins"],
            json!(row.timeout_in_mins),
            "case {case:?}: the start call must report the resolved batch timeout, got \
             {started}"
        );
        assert_eq!(
            started["buildBatchConfig"]["restrictions"]["maximumBuildsAllowed"],
            json!(row.maximum_builds),
            "case {case:?}: the start call must report the resolved build restriction, \
             got {started}"
        );

        let settled = settled_batch(&router, case, &id).await;

        assert_eq!(
            settled["buildBatchConfig"]["timeoutInMins"],
            json!(row.timeout_in_mins),
            "case {case:?}: the stored batch must report the resolved batch timeout, got \
             {settled}"
        );
        assert_eq!(
            settled["buildBatchConfig"]["restrictions"]["maximumBuildsAllowed"],
            json!(row.maximum_builds),
            "case {case:?}: the stored batch must report the resolved build restriction, \
             got {settled}"
        );

        // RULE 2, the outcome: the batch takes the status its own builds settled on, and
        // sits in the phase of that name.
        assert_eq!(
            settled["buildBatchStatus"],
            json!(row.outcome),
            "case {case:?}: expected the batch to settle {}, got {settled}",
            row.outcome
        );
        assert_eq!(
            settled["currentPhase"],
            json!(row.outcome),
            "case {case:?}: a settled batch sits in the phase of its own outcome, got \
             {settled}"
        );
        assert!(
            settled["endTime"].is_number(),
            "case {case:?}: a settled batch carries an endTime, got {settled}"
        );

        // RULE 2, the walk: the phases the batch went through, in order, each finished
        // one carrying the interval it took.
        let expected_phases: Vec<(String, Option<String>)> = vec![
            ("SUBMITTED".to_string(), Some("SUCCEEDED".to_string())),
            (
                "DOWNLOAD_BATCHSPEC".to_string(),
                Some("SUCCEEDED".to_string()),
            ),
            ("IN_PROGRESS".to_string(), Some(row.outcome.to_string())),
            (
                "COMBINE_ARTIFACTS".to_string(),
                Some(row.outcome.to_string()),
            ),
            (row.outcome.to_string(), None),
        ];
        assert_eq!(
            phase_pairs(&settled),
            expected_phases,
            "case {case:?}: expected the batch's whole phase walk, got {settled}"
        );
        let phases = settled["phases"].as_array().cloned().unwrap_or_default();
        for phase in phases.iter().take(expected_phases.len() - 1) {
            let phase_type = phase["phaseType"].as_str().unwrap_or_default().to_string();
            let start = phase["startTime"].as_f64();
            let end = phase["endTime"].as_f64();
            assert!(
                start.is_some() && end.is_some(),
                "case {case:?}: finished phase {phase_type} must carry both instants, got \
                 {phase}"
            );
            assert!(
                end >= start,
                "case {case:?}: finished phase {phase_type} must not end before it \
                 started, got {phase}"
            );
            assert!(
                phase["durationInSeconds"].as_i64().is_some_and(|seconds| seconds >= 0),
                "case {case:?}: finished phase {phase_type} must report how long it took, \
                 got {phase}"
            );
        }

        // RULE 2, the groups: the batch's own source download first, then the declared
        // builds in the order the batchspec declared them.
        let expected_identifiers: Vec<String> =
            std::iter::once(state::SOURCE_DOWNLOAD_IDENTIFIER.to_string())
                .chain(row.declared.iter().map(|id| (*id).to_string()))
                .collect();
        assert_eq!(
            group_identifiers(&settled),
            expected_identifiers,
            "case {case:?}: expected the source download beside every declared build, got \
             {settled}"
        );

        let expected_type = state::wire_artifacts_type(
            row.artifacts["type"].as_str().unwrap_or_default(),
        );
        for group in settled["buildGroups"].as_array().cloned().unwrap_or_default() {
            let identifier = group["identifier"].as_str().unwrap_or_default().to_string();
            let case = format!("{case}, group {identifier:?}");
            let summary = group["currentBuildSummary"].clone();
            let arn = summary["arn"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                summary["buildStatus"],
                json!(row.outcome),
                "case {case:?}: the group must report the status of the build it named, \
                 got {summary}"
            );
            assert!(
                summary["requestedOn"].is_number(),
                "case {case:?}: the group must report when its build was requested, got \
                 {summary}"
            );
            // The artifacts the build RESOLVED: the destination's type in the token form
            // the service states, and the group the build ran for as the identifier.
            assert_eq!(
                summary["primaryArtifact"]["type"],
                json!(expected_type),
                "case {case:?}: the resolved artifact must name the destination's type, \
                 got {summary}"
            );
            assert_eq!(
                summary["primaryArtifact"]["identifier"],
                json!(identifier),
                "case {case:?}: the resolved artifact must name the group it was built \
                 for, got {summary}"
            );
            assert_eq!(
                summary["secondaryArtifacts"].as_array(),
                Some(&Vec::new()),
                "case {case:?}: a batch of a project with no secondary artifacts resolves \
                 none, got {summary}"
            );
            // BACKED: the ARN the group publishes resolves to a real build of the same
            // project that names this batch.
            let (status, builds) = call(&router, "BatchGetBuilds", json!({"ids": [arn.clone()]}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: BatchGetBuilds expected 200, got {status} with {builds}"
            );
            assert_eq!(
                builds["builds"][0]["arn"],
                json!(arn),
                "case {case:?}: the build the group names must be retrievable, got {builds}"
            );
            assert_eq!(
                builds["builds"][0]["buildBatchArn"],
                json!(batch_arn),
                "case {case:?}: the build must name the batch it belongs to, got {builds}"
            );
        }

        // RULE 3: the destination the batch runs with.
        assert_eq!(
            settled["artifacts"]["type"],
            json!(expected_type),
            "case {case:?}: the batch must report its destination's type, got {settled}"
        );
        assert_eq!(
            settled["artifacts"]["location"],
            json!(row.location),
            "case {case:?}: the batch must report its destination's location, got {settled}"
        );

        // The queue timeout the batch runs with: the project's resolved default, since no
        // row asks for another.
        assert_eq!(
            settled["queuedTimeoutInMinutes"],
            json!(state::DEFAULT_QUEUED_TIMEOUT_MINUTES),
            "case {case:?}: the batch must report the queue timeout it runs with, got \
             {settled}"
        );

        // Reading a settled batch again answers the same record: materialising the owed
        // transitions is idempotent.
        let again = settled_batch(&router, case, &id).await;
        assert_eq!(
            again, settled,
            "case {case:?}: a second read of a settled batch must answer the same record"
        );
    }
}
