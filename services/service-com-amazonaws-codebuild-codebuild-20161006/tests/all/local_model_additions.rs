// SPDX-License-Identifier: CC0-1.0

//! The members CodeBuild answers with that the PUBLISHED model does not declare, and the
//! overlay they are declared from.
//!
//! RULE: a member the service puts on the wire is reported by the operation that reports
//! it, whether or not the published model declares the member — and a member the published
//! model does not declare is declared in this crate from the overlay model beside the
//! snapshot (`smithy-model/local-additions.json`, see `smithy-model/LOCAL-ADDITIONS.md`),
//! which restates what the snapshot declares and adds only that member. Three members are
//! in that position: the destination `type` of a build record's `artifacts`, the
//! `customEntrypoint` of a provisioned build's `environment`, and the
//! `webhookDeletedStatus` a project delete answers with.
//!
//! The wire halves are driven end to end through the harness `project_build_lifecycle.rs`
//! keeps: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1. The
//! rows differ along what each rule generalises over — the destination type a project is
//! configured with, the compute the environment asks for, whether the build runs
//! privileged, and how a project comes to carry a webhook or not.
//!
//! The last test pins the DECLARATION rather than the wire: everything the overlay restates
//! of the snapshot is compared against it, shape by shape, so a refreshed snapshot that
//! declares one of these members, adds a member the overlay does not restate, or changes a
//! target, value set or constraint fails here rather than silently narrowing what this
//! service reports.

use serde_json::{json, Map, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{
    call, create_project, emulator, settled_build, source_of, start_build,
};

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the local declarations are checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json"
));

/// The overlay model the local declarations are generated from, embedded for the same
/// reason: what it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The shapes the overlay restates in order to add a member to them, with the one member
/// each of them adds.
const LOCAL_MEMBERS: &[(&str, &str)] = &[
    ("com.amazonaws.codebuild#BuildArtifacts", "type"),
    ("com.amazonaws.codebuild#ProjectEnvironment", "customEntrypoint"),
    (
        "com.amazonaws.codebuild#DeleteProjectOutput",
        "webhookDeletedStatus",
    ),
];

/// The structure shapes the overlay declares only so codegen can classify a member that
/// targets one: their own members are generated from the snapshot, beside every other
/// modelled shape, so the overlay restates none of them and none of them can drift.
const TARGET_STUBS: &[&str] = &[
    "com.amazonaws.codebuild#ComputeConfiguration",
    "com.amazonaws.codebuild#ProjectFleet",
    "com.amazonaws.codebuild#EnvironmentVariable",
    "com.amazonaws.codebuild#RegistryCredential",
    "com.amazonaws.codebuild#DockerServer",
];

/// `CreateProject` members for a project whose builds run a buildspec with one command, on
/// `compute` with `privileged` set, writing to `artifacts`.
fn project_with(compute: &str, privileged: bool, artifacts: Value) -> Value {
    json!({
        "source": {
            "type": "NO_SOURCE",
            "buildspec": "version: 0.2\nphases:\n  build:\n    commands:\n      - echo built\n",
        },
        "environment": {
            "type": "LINUX_CONTAINER",
            "image": "aws/codebuild/standard:7.0",
            "computeType": compute,
            "privilegedMode": privileged,
        },
        "artifacts": artifacts,
    })
}

/// RULE: a build record states the destination type it carries in the token form the
/// service reports an artifact's type in, and a build whose environment has been
/// provisioned reports that its container was started with the service's own entrypoint —
/// neither of which the published model declares a member for.
///
/// The rows differ along what the rule generalises over: which destination type is
/// configured (each value `ArtifactsType` declares, from
/// `smithy_query(query="shape",shape="ArtifactsType")`: CODEPIPELINE, S3, NO_ARTIFACTS),
/// whether the destination names a location, the compute the environment asks for, and
/// whether the build runs privileged.
#[tokio::test]
async fn a_build_record_reports_the_members_the_published_model_omits() {
    let router = emulator();
    // (case, the compute type, privileged, the project's `artifacts`, the reported type)
    let rows: Vec<(&str, &str, bool, Value, &str)> = vec![
        (
            "no destination at all, on the smallest compute",
            "BUILD_GENERAL1_SMALL",
            false,
            json!({"type": "NO_ARTIFACTS"}),
            "no_artifacts",
        ),
        (
            "a destination in the object store, on a larger compute",
            "BUILD_GENERAL1_LARGE",
            false,
            json!({"type": "S3", "location": "local-additions-destination"}),
            "s3",
        ),
        (
            "a destination the pipeline owns, privileged",
            "BUILD_GENERAL1_MEDIUM",
            true,
            json!({"type": "CODEPIPELINE"}),
            "codepipeline",
        ),
    ];

    for (index, (case, compute, privileged, artifacts, expected_type)) in rows.iter().enumerate() {
        let project = format!("local-additions-build-{index}");
        create_project(
            &router,
            &project,
            project_with(compute, *privileged, artifacts.clone()),
        )
        .await;

        // The start: the destination is already stated, and no environment has been
        // provisioned for the build yet, so it reports no entrypoint of its own.
        let started = start_build(&router, json!({"projectName": project})).await;
        assert_eq!(
            started["artifacts"]["type"],
            json!(expected_type),
            "case {case:?}: StartBuild expected artifacts.type {expected_type:?}, got {}",
            started["artifacts"]
        );
        assert!(
            started["environment"]["customEntrypoint"].is_null(),
            "case {case:?}: a build with no environment yet reports no custom entrypoint, \
             got {}",
            started["environment"]
        );

        // The settled record, read back through `BatchGetBuilds`: the same destination, and
        // the environment the build RAN IN.
        let settled = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            settled["artifacts"]["type"],
            json!(expected_type),
            "case {case:?}: BatchGetBuilds expected artifacts.type {expected_type:?}, got {}",
            settled["artifacts"]
        );
        assert_eq!(
            settled["environment"]["customEntrypoint"],
            json!(true),
            "case {case:?}: a provisioned environment is reported, got {}",
            settled["environment"]
        );
        // The configured destination keeps the MODELLED, upper-case value: the token form
        // is how a record REPORTS an artifact, not how one is configured.
        let stored_project = call(&router, "BatchGetProjects", json!({"names": [&project]})).await;
        assert_eq!(
            stored_project.1["projects"][0]["artifacts"]["type"],
            artifacts["type"],
            "case {case:?}: the project keeps the configured destination type, got {}",
            stored_project.1["projects"][0]["artifacts"]
        );
    }
}

/// Hold an access token for `server_type`, which a webhook CodeBuild registers with a
/// repository provider needs.
async fn import_source_credentials(router: &MultiServiceRouter, server_type: &str) {
    let (status, body) = call(
        router,
        "ImportSourceCredentials",
        json!({
            "serverType": server_type,
            "authType": "PERSONAL_ACCESS_TOKEN",
            "token": "an-access-token",
            "shouldOverwrite": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ImportSourceCredentials ({server_type}): expected 200, got {status} with {body}"
    );
}

/// RULE: the delete of a project states what became of the webhook record CodeBuild kept
/// against it — a member the published `DeleteProjectOutput` declares none of.
///
/// The rows differ along how the project came to carry a webhook or not: a source type a
/// webhook can be created for with one connected, the same source type with none, and a
/// source type no webhook can be created for at all.
#[tokio::test]
async fn deleting_a_project_reports_the_disposition_the_published_model_omits() {
    // (case, the project's source type, whether a webhook is connected)
    let rows: Vec<(&str, &str, bool)> = vec![
        (
            "a project carrying a webhook",
            state::WEBHOOK_SOURCE_TYPES[0],
            true,
        ),
        (
            "a project of the same source type carrying none",
            state::WEBHOOK_SOURCE_TYPES[0],
            false,
        ),
        (
            "a project no webhook can be created for",
            state::NO_SOURCE_TYPE,
            false,
        ),
    ];

    for (index, (case, source_type, with_webhook)) in rows.iter().enumerate() {
        // A fresh emulator per row: a row's state must not depend on the one before.
        let router = emulator();
        let project = format!("local-additions-delete-{index}");
        create_project(&router, &project, json!({"source": source_of(source_type)})).await;
        if *with_webhook {
            import_source_credentials(&router, source_type).await;
            let (status, body) = call(&router, "CreateWebhook", json!({"projectName": project}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateWebhook expected 200, got {status} with {body}"
            );
        }

        let expected = state::webhook_deleted_status(*with_webhook);
        let (status, deleted) = call(&router, "DeleteProject", json!({"name": project})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteProject expected 200, got {status} with {deleted}"
        );
        assert_eq!(
            deleted["webhookDeletedStatus"],
            json!(expected),
            "case {case:?}: expected the disposition {expected:?}, got {deleted}"
        );
    }
}

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Value {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    Value::Object(
        parsed["shapes"]
            .as_object()
            .expect("a model document must carry a shapes object")
            .clone(),
    )
}

/// `declaration` with every documentation trait dropped, recursively — and with a traits
/// object left empty by that dropped too, so a member the snapshot documents and nothing
/// else compares equal to the same member restated with no traits at all.
///
/// Documentation is prose: the overlay restates what codegen READS — targets, types, enum
/// values and constraint traits — so comparing declarations has to ignore the one trait
/// that carries no behaviour.
fn without_docs(declaration: &Value) -> Value {
    match declaration {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "smithy.api#documentation")
                .map(|(key, value)| (key.clone(), without_docs(value)))
                .filter(|(key, value)| {
                    key.as_str() != "traits" || !value.as_object().is_some_and(Map::is_empty)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_docs).collect()),
        other => other.clone(),
    }
}

/// `declaration` without the member `added`, when it adds one — what the overlay restates
/// of the snapshot, which is what has to match it.
fn without_added(declaration: &Value, added: Option<&str>) -> Value {
    let Some(added) = added else {
        return declaration.clone();
    };
    let mut restated = declaration.clone();
    if let Some(members) = restated["members"].as_object_mut() {
        members.remove(added);
    }
    restated
}

#[test]
fn the_overlay_restates_the_snapshot_exactly_but_for_the_members_it_adds() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    for (shape_id, declared) in restated {
        let modelled = &snapshot[shape_id];
        assert!(
            !modelled.is_null(),
            "the overlay restates {shape_id}, which the snapshot does not declare"
        );
        if TARGET_STUBS.contains(&shape_id.as_str()) {
            // A stub restates nothing: only the shape's TYPE, so a member targeting it is
            // classified the way the snapshot classifies it.
            assert_eq!(
                declared["type"], modelled["type"],
                "{shape_id} must be declared with the type the snapshot gives it, got {}",
                declared["type"]
            );
            assert!(
                declared.get("members").is_none(),
                "{shape_id} is declared only for a member to target, so it must restate no \
                 member of its own; it declares {}",
                declared["members"]
            );
            continue;
        }

        let added = LOCAL_MEMBERS
            .iter()
            .find(|(carrier, _)| *carrier == shape_id.as_str())
            .map(|(_, member)| *member);
        if let Some(added) = added {
            assert!(
                modelled["members"]
                    .as_object()
                    .is_none_or(|members| !members.contains_key(added)),
                "the snapshot now declares {added} on {shape_id}, so the local declaration \
                 should give way to the modelled member"
            );
            assert!(
                declared["members"]
                    .as_object()
                    .is_some_and(|members| members.contains_key(added)),
                "the overlay restates {shape_id} in order to add {added}, which it does not \
                 declare"
            );
        }
        // Everything else the overlay states for the shape — its type, its own traits, and
        // every member it restates with that member's target and constraints — must be
        // exactly what the snapshot states.
        assert_eq!(
            without_docs(&without_added(declared, added)),
            without_docs(modelled),
            "{shape_id} must be restated as the snapshot declares it, got {}",
            without_added(declared, added)
        );
    }

    // Every shape a local declaration is of is restated: a carrier left out of the overlay
    // would silently lose the member.
    for (carrier, member) in LOCAL_MEMBERS {
        assert!(
            restated.contains_key(*carrier),
            "the overlay must restate {carrier}, the shape the local {member} is declared on"
        );
    }
}
