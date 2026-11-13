// SPDX-License-Identifier: CC0-1.0

//! Cover for the preconditions `StartBuildBatch` holds a request to before it mints
//! anything, driven over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.StartBuildBatch` on awsJson1_1.
//!
//! Two rules, both answered with the operation's modelled `InvalidInputException`
//! (client, HTTP 400) and with NOTHING persisted:
//!
//! 1. A batch build runs under batch build options, and EITHER source states them: the
//!    project's own `buildBatchConfig` — the object "that defines the batch build options
//!    for the project" — or the `buildBatchConfigOverride` the start call carries for
//!    this one batch, the way every other `*Override` member overlays the stored project.
//!    Only a start where NEITHER states one is refused.
//! 2. A batch build configuration's `timeoutInMins` is held to the 5..2160-minute
//!    window the model pins on the sibling member that times the builds of the same
//!    batch (`buildTimeoutInMinutesOverride` -> `BuildTimeOut`
//!    `@range(min: 5, max: 2160)`); `timeoutInMins` itself targets the unconstrained
//!    `WrapperInt`, so the framework's constraint walk refuses nothing for it and the
//!    service is what holds it to the window. The rule holds wherever a
//!    `ProjectBuildBatchConfig` is accepted: `CreateProject$buildBatchConfig`,
//!    `UpdateProject$buildBatchConfig` and
//!    `StartBuildBatch$buildBatchConfigOverride`.
//!
//! Each refusal is read back against the store through `ListBuildBatchesForProject`
//! and `BatchGetProjects`, so "nothing persisted" is asserted through the implemented
//! readers rather than inferred from the status.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::project_build_lifecycle::{
    batch_enabled, call, create_project, create_project_body, emulator, error_type, listed_ids,
    merged,
};

/// The sentence CodeBuild states for a `timeoutInMins` outside the window — its text
/// as the service words it, pinned here rather than read from the implementation, so a
/// change of wording fails this suite. The window's bounds are the ones the model
/// publishes on `BuildTimeOut`, asserted against the service's own constants below.
const TIMEOUT_WINDOW_SENTENCE: &str = "TimeoutInMins must be between 5 and 2160";

/// The sentence CodeBuild states for a project that carries no batch build options,
/// pinned the same way.
const NOT_BATCH_ENABLED_SENTENCE: &str =
    "BatchConfig must be configured on the project to use the BuildBatch feature";

/// The published window the sentence above states, as the service holds a request to
/// it: the test's text and the service's bounds have to agree.
#[test]
fn the_published_window_is_the_one_the_sentence_states() {
    assert_eq!(
        (
            state::BATCH_TIMEOUT_MIN_MINUTES,
            state::BATCH_TIMEOUT_MAX_MINUTES
        ),
        (5, 2160),
        "the batch timeout window must be the 5..2160 minutes the model publishes on \
         BuildTimeOut"
    );
}

/// The batch build ids `ListBuildBatchesForProject` reports for `project`.
async fn listed_batches(router: &MultiServiceRouter, project: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListBuildBatchesForProject",
        json!({"projectName": project}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListBuildBatchesForProject {project:?}: expected 200, got {status} with {body}"
    );
    listed_ids(&body)
}

/// The stored project `name`, through `BatchGetProjects`.
async fn stored_project(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [name]})).await;
    assert_eq!(
        status, 200,
        "BatchGetProjects {name:?}: expected 200, got {status} with {body}"
    );
    body["projects"][0].clone()
}

/// RULE: a batch build needs batch build options, which EITHER the project's own
/// `buildBatchConfig` or the start call's `buildBatchConfigOverride` may state;
/// `StartBuildBatch` where neither states one is refused with `InvalidInputException` and
/// mints no batch build.
///
/// The table spans the forms a project's batch configuration takes — absent, carrying
/// only a service role, carrying only a timeout, carrying every member — crossed with the
/// request forms: the bare request, one carrying an idempotency token, and one carrying a
/// `buildBatchConfigOverride`, which states the batch options a project that has none
/// never got.
#[tokio::test]
async fn start_build_batch_requires_the_project_to_carry_a_batch_configuration() {
    // (case, the project's CreateProject members, whether the PROJECT is batch-enabled)
    let projects: Vec<(&str, Value, bool)> = vec![
        ("no buildBatchConfig at all", json!({}), false),
        (
            "a buildBatchConfig carrying only a service role",
            json!({"buildBatchConfig": {
                "serviceRole": "arn:aws:iam::000000000000:role/service-role/batch",
            }}),
            true,
        ),
        (
            "a buildBatchConfig carrying only a timeout",
            json!({"buildBatchConfig": {"timeoutInMins": 5}}),
            true,
        ),
        (
            "a buildBatchConfig carrying every member",
            json!({"buildBatchConfig": {
                "serviceRole": "arn:aws:iam::000000000000:role/service-role/batch",
                "combineArtifacts": true,
                "timeoutInMins": 2160,
                "batchReportMode": "REPORT_AGGREGATED_BATCH",
                "restrictions": {
                    "maximumBuildsAllowed": 4,
                    "computeTypesAllowed": ["BUILD_GENERAL1_SMALL"],
                },
            }}),
            true,
        ),
    ];

    for (index, (case, configuration, batch_capable)) in projects.into_iter().enumerate() {
        // A fresh emulator per row: a row's state must not depend on the one before.
        let router = emulator();
        let project = format!("batch-cap-{index}");
        create_project(&router, &project, configuration).await;

        // (what the request adds to `projectName`, whether the REQUEST states the batch
        // options itself)
        let requests: [(&str, Value, bool); 3] = [
            ("the bare request", json!({}), false),
            (
                "the request carrying an idempotency token",
                json!({"idempotencyToken": "token-1"}),
                false,
            ),
            (
                "the request carrying a batch configuration override",
                json!({"buildBatchConfigOverride": {
                    "serviceRole": "arn:aws:iam::000000000000:role/service-role/batch",
                    "timeoutInMins": 30,
                }}),
                true,
            ),
        ];
        let mut accepted_requests = 0;
        for (form, extra, request_states_options) in requests {
            let request = merged(json!({"projectName": project.clone()}), extra);
            let (status, body) = call(&router, "StartBuildBatch", request.clone()).await;
            if batch_capable || request_states_options {
                accepted_requests += 1;
                assert_eq!(
                    status, 200,
                    "case {case:?} / {form}: a batch whose options either source states \
                     must be buildable, got {status} with {body}"
                );
                assert_eq!(
                    body["buildBatch"]["buildBatchStatus"],
                    json!("IN_PROGRESS"),
                    "case {case:?} / {form}: the started batch must be in flight, got {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?} / {form}: a batch no source states options for must be \
                     refused, got {status} with {body}"
                );
                assert_eq!(
                    error_type(&body),
                    "InvalidInputException",
                    "case {case:?} / {form}: expected InvalidInputException, got {body}"
                );
                assert_eq!(
                    body["message"],
                    json!(NOT_BATCH_ENABLED_SENTENCE),
                    "case {case:?} / {form}: expected CodeBuild's not-batch-enabled \
                     sentence, got {body}"
                );
            }
        }

        // What the store holds afterwards: a batch build per accepted request, and none
        // at all for a project every request was refused for. The token row replays the
        // first row's request, so an accepted project holds one batch per DISTINCT
        // request.
        let listed = listed_batches(&router, &project).await;
        assert_eq!(
            listed.len(),
            accepted_requests,
            "case {case:?}: expected {accepted_requests} batch build(s) to be listed for \
             the project, got {listed:?}"
        );
    }
}

/// RULE: a `timeoutInMins` outside the published 5..2160-minute window is refused
/// wherever a `ProjectBuildBatchConfig` is accepted, with the operation's modelled
/// `InvalidInputException` and nothing written.
///
/// The table spans the window: each published bound, one minute inside each, one
/// minute outside each, zero, a negative value, and a value far above the ceiling —
/// driven through all three members that take such a configuration.
#[tokio::test]
async fn a_batch_timeout_outside_the_published_window_is_refused_wherever_it_is_given() {
    let min = state::BATCH_TIMEOUT_MIN_MINUTES;
    let max = state::BATCH_TIMEOUT_MAX_MINUTES;
    // (case, the timeoutInMins asked for, whether it is inside the window)
    let timeouts: Vec<(&str, i64, bool)> = vec![
        ("the published minimum", i64::from(min), true),
        ("one minute inside the minimum", i64::from(min) + 1, true),
        ("the published maximum", i64::from(max), true),
        ("one minute inside the maximum", i64::from(max) - 1, true),
        ("one minute below the minimum", i64::from(min) - 1, false),
        ("one minute above the maximum", i64::from(max) + 1, false),
        ("zero", 0, false),
        ("a negative timeout", -100, false),
        ("far above the ceiling", i64::from(max) * 10, false),
    ];

    for (index, (case, minutes, inside)) in timeouts.into_iter().enumerate() {
        let router = emulator();
        let config = json!({"buildBatchConfig": {"timeoutInMins": minutes}});

        // 1. CreateProject: the configuration is refused before the project exists.
        let created = format!("batch-timeout-create-{index}");
        let create = create_project_body(&created, config.clone());
        let (status, body) = call(&router, "CreateProject", create).await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?} / CreateProject: expected 200, got {status} with {body}"
            );
            assert_eq!(
                body["project"]["buildBatchConfig"]["timeoutInMins"],
                json!(minutes),
                "case {case:?} / CreateProject: the stored timeout must be the one asked \
                 for, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} / CreateProject: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "InvalidInputException",
                "case {case:?} / CreateProject: expected InvalidInputException, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(TIMEOUT_WINDOW_SENTENCE),
                "case {case:?} / CreateProject: expected CodeBuild's timeout-window \
                 sentence, got {body}"
            );
            assert!(
                stored_project(&router, &created).await.is_null(),
                "case {case:?} / CreateProject: a refused create must store no project"
            );
        }

        // 2. UpdateProject: the configuration is refused and the project stays as it
        //    was, batch options included.
        let updated = format!("batch-timeout-update-{index}");
        let before = create_project(&router, &updated, batch_enabled()).await;
        let (status, body) = call(
            &router,
            "UpdateProject",
            merged(json!({"name": updated.clone()}), config.clone()),
        )
        .await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?} / UpdateProject: expected 200, got {status} with {body}"
            );
            assert_eq!(
                body["project"]["buildBatchConfig"]["timeoutInMins"],
                json!(minutes),
                "case {case:?} / UpdateProject: the stored timeout must be the one asked \
                 for, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} / UpdateProject: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "InvalidInputException",
                "case {case:?} / UpdateProject: expected InvalidInputException, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(TIMEOUT_WINDOW_SENTENCE),
                "case {case:?} / UpdateProject: expected CodeBuild's timeout-window \
                 sentence, got {body}"
            );
            assert_eq!(
                stored_project(&router, &updated).await,
                before,
                "case {case:?} / UpdateProject: a refused update must leave the project \
                 exactly as it was"
            );
        }

        // 3. StartBuildBatch's override: the batch is refused and none is persisted.
        let started = format!("batch-timeout-start-{index}");
        create_project(&router, &started, batch_enabled()).await;
        let (status, body) = call(
            &router,
            "StartBuildBatch",
            json!({
                "projectName": started.clone(),
                "buildBatchConfigOverride": {
                    "serviceRole": "arn:aws:iam::000000000000:role/service-role/batch",
                    "timeoutInMins": minutes,
                },
            }),
        )
        .await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?} / StartBuildBatch: expected 200, got {status} with {body}"
            );
            assert_eq!(
                body["buildBatch"]["buildBatchConfig"]["timeoutInMins"],
                json!(minutes),
                "case {case:?} / StartBuildBatch: the batch must run with the override it \
                 was given, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} / StartBuildBatch: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "InvalidInputException",
                "case {case:?} / StartBuildBatch: expected InvalidInputException, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(TIMEOUT_WINDOW_SENTENCE),
                "case {case:?} / StartBuildBatch: expected CodeBuild's timeout-window \
                 sentence, got {body}"
            );
        }
        let listed = listed_batches(&router, &started).await;
        let expected = usize::from(inside);
        assert_eq!(
            listed.len(),
            expected,
            "case {case:?} / StartBuildBatch: expected {expected} batch build(s) listed \
             for the project, got {listed:?}"
        );
    }
}

/// RULE: a batch configuration that states NO timeout is accepted — the member is
/// optional — wherever one is given, and the batch that starts under it reports the
/// configuration it RUNS WITH: the documented defaults resolved for the members the
/// configuration left unstated, and every stated member as stated.
#[tokio::test]
async fn a_batch_configuration_that_states_no_timeout_is_accepted() {
    let router = emulator();
    // (case, the buildBatchConfig members, the maximumBuildsAllowed the batch must run
    // under)
    let configs: [(&str, Value, i32); 3] = [
        (
            "an empty configuration",
            json!({}),
            state::DEFAULT_MAXIMUM_BUILDS_ALLOWED,
        ),
        (
            "a configuration with a service role only",
            json!({"serviceRole": "arn:aws:iam::000000000000:role/service-role/batch"}),
            state::DEFAULT_MAXIMUM_BUILDS_ALLOWED,
        ),
        (
            "a configuration with restrictions only",
            json!({"restrictions": {"maximumBuildsAllowed": 2}}),
            2,
        ),
    ];

    for (index, (case, config, maximum_builds)) in configs.into_iter().enumerate() {
        let project = format!("batch-no-timeout-{index}");
        create_project(
            &router,
            &project,
            json!({"buildBatchConfig": config.clone()}),
        )
        .await;
        let (status, body) = call(
            &router,
            "StartBuildBatch",
            json!({
                "projectName": project.clone(),
                "buildBatchConfigOverride": config.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a configuration stating no timeout must be accepted, got \
             {status} with {body}"
        );
        assert_eq!(
            body["buildBatch"]["buildBatchConfig"]["timeoutInMins"],
            json!(state::DEFAULT_BATCH_TIMEOUT_MINUTES),
            "case {case:?}: a configuration stating no timeout must run to the resolved \
             default, got {body}"
        );
        assert_eq!(
            body["buildBatch"]["buildBatchConfig"]["restrictions"]["maximumBuildsAllowed"],
            json!(maximum_builds),
            "case {case:?}: expected the batch to run under {maximum_builds} builds \
             allowed, got {body}"
        );
    }
}
