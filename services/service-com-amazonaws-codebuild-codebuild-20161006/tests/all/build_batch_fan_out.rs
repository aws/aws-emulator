// SPDX-License-Identifier: CC0-1.0

//! Cover for a batch build's FAN-OUT and for how a batch read names what it could not
//! resolve, driven over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! Two rules:
//!
//! 1. `StartBuildBatch` starts "a batch build for a project", and which builds that batch
//!    is made of is declared by the `batch` section of the buildspec the batch runs with.
//!    So the service itself — with no further call from the client — starts one build per
//!    declared identifier and publishes it in that identifier's build group
//!    (`BuildGroup$currentBuildSummary`), where each build is a real build of the same
//!    project that `BatchGetBuilds` resolves on its own and that names the batch it
//!    belongs to through `Build$buildBatchArn`. Ahead of them all stands the batch's OWN
//!    group — `state::SOURCE_DOWNLOAD_IDENTIFIER`, the source download every declared
//!    build is then built from — which the batchspec does not declare and the batch
//!    reports first. A buildspec declaring no `batch` section declares no build at all,
//!    so the batch carries no group.
//! 2. A batch build is named by its ARN in every answer this service gives, so
//!    `BatchGetBuildBatches` reports an identifier it could not resolve in the same ARN
//!    form — which is what lets a client diff the ARNs it asked for against
//!    `buildBatchesNotFound`.
//!
//! Each table spans the class: the batchspec rows differ along the declared build type
//! (`build-list`, `build-graph`), the count of identifiers, the dependencies between
//! them and where the buildspec came from (the project, or this request's override); the
//! not-found rows differ along the form of the identifier asked for.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{
    batch_enabled, call, create_project, emulator, merged, REGION,
};

/// A `CreateProject` source whose inline buildspec is `buildspec`.
fn source_with(buildspec: &str) -> Value {
    json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}})
}

/// A buildspec whose `batch` section declares `build-list` entries for `identifiers`.
fn build_list_spec(identifiers: &[&str]) -> String {
    let mut spec = String::from("version: 0.2\nbatch:\n  fast-fail: false\n  build-list:\n");
    for identifier in identifiers {
        spec.push_str(&format!("    - identifier: {identifier}\n"));
    }
    spec.push_str("phases:\n  build:\n    commands:\n      - echo building\n");
    spec
}

/// The build groups a batch answer carries, as (identifier, the ARN of its current
/// build) pairs in the order they are reported.
fn reported_groups(batch: &Value) -> Vec<(String, String)> {
    batch["buildGroups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .map(|group| {
                    (
                        group["identifier"].as_str().unwrap_or_default().to_string(),
                        group["currentBuildSummary"]["arn"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Start a batch build and hand back the batch, asserting the call succeeded.
async fn start_build_batch(router: &MultiServiceRouter, request: Value) -> Value {
    let (status, body) = call(router, "StartBuildBatch", request.clone()).await;
    assert_eq!(
        status, 200,
        "StartBuildBatch {request}: expected 200, got {status} with {body}"
    );
    body["buildBatch"].clone()
}

/// RULE 1: a batch build fans out into the source download it runs first and one build
/// per identifier its batchspec declares, each published in its own build group and each a
/// real, independently retrievable build of the same project that names the batch it
/// belongs to.
#[tokio::test]
async fn a_batch_build_fans_out_into_the_builds_its_batchspec_declares() {
    // (case, the project's buildspec, this request's buildspecOverride, the identifiers
    // the batchspec DECLARES, in order, with the identifiers each one waits for)
    type Row = (
        &'static str,
        String,
        Option<String>,
        Vec<(&'static str, Vec<&'static str>)>,
    );
    let graph_spec = "version: 0.2\n\
         batch:\n  \
           build-graph:\n    \
             - identifier: base\n    \
             - identifier: middle\n      \
               depends-on:\n        \
                 - base\n    \
             - identifier: leaf\n      \
               depends-on:\n        \
                 - middle\n      \
               ignore-failure: true\n"
        .to_string();
    let rows: Vec<Row> = vec![
        (
            "a build-list of two identifiers",
            build_list_spec(&["build1", "build2"]),
            None,
            vec![("build1", vec![]), ("build2", vec![])],
        ),
        (
            "a build-list of one identifier",
            build_list_spec(&["only"]),
            None,
            vec![("only", vec![])],
        ),
        (
            "a build-list of five identifiers",
            build_list_spec(&["a", "b", "c", "d", "e"]),
            None,
            vec![
                ("a", vec![]),
                ("b", vec![]),
                ("c", vec![]),
                ("d", vec![]),
                ("e", vec![]),
            ],
        ),
        (
            "a build-graph whose groups depend on each other",
            graph_spec,
            None,
            vec![
                ("base", vec![]),
                ("middle", vec!["base"]),
                ("leaf", vec!["middle"]),
            ],
        ),
        (
            "a batchspec that arrives as this request's override",
            "version: 0.2\n".to_string(),
            Some(build_list_spec(&["overridden1", "overridden2"])),
            vec![("overridden1", vec![]), ("overridden2", vec![])],
        ),
        (
            "a buildspec declaring no batch section",
            "version: 0.2\nphases:\n  build:\n    commands:\n      - echo plain\n".to_string(),
            None,
            vec![],
        ),
        (
            "a buildspec whose batch section declares no build type",
            "version: 0.2\nbatch:\n  fast-fail: true\n".to_string(),
            None,
            vec![],
        ),
        (
            "a buildspec that is not YAML at all",
            "version: 0.2\n\tbatch: [".to_string(),
            None,
            vec![],
        ),
        (
            // A batchspec whose aliases would expand without bound is not read: the
            // parse is bounded, so the batch declares nothing rather than costing the
            // request the memory the expansion would.
            "a batchspec built out of YAML aliases",
            "version: 0.2\n\
             batch: &a\n  \
               build-list: &b\n    \
                 - identifier: one\n\
             repeated: *a\n\
             again: *b\n"
                .to_string(),
            None,
            vec![],
        ),
        (
            // Nesting past the bound the parse is given.
            "a batchspec nested deeper than the parse reads",
            format!("version: 0.2\nbatch: {}", "[".repeat(200)),
            None,
            vec![],
        ),
    ];

    for (index, (case, buildspec, override_spec, declared)) in rows.into_iter().enumerate() {
        // The groups the batch publishes: its own source download first, then the
        // declared ones. A batchspec that declared nothing publishes neither.
        let expected: Vec<(&'static str, Vec<&'static str>)> = if declared.is_empty() {
            Vec::new()
        } else {
            std::iter::once((state::SOURCE_DOWNLOAD_IDENTIFIER, Vec::new()))
                .chain(declared)
                .collect()
        };
        let router = emulator();
        let project = format!("fan-out-{index}");
        create_project(
            &router,
            &project,
            merged(source_with(&buildspec), batch_enabled()),
        )
        .await;
        let mut request = json!({"projectName": project});
        if let (Value::Object(members), Some(spec)) = (&mut request, override_spec) {
            members.insert("buildspecOverride".to_string(), json!(spec));
        }
        let batch = start_build_batch(&router, request).await;
        let batch_arn = batch["arn"].as_str().unwrap_or_default().to_string();
        let groups = reported_groups(&batch);

        if expected.is_empty() {
            assert!(
                batch["buildGroups"].is_null(),
                "case {case:?}: a batch that declared no build carries no build group, \
                 got {batch}"
            );
            continue;
        }

        let identifiers: Vec<String> = groups.iter().map(|(id, _)| id.clone()).collect();
        let expected_identifiers: Vec<String> =
            expected.iter().map(|(id, _)| (*id).to_string()).collect();
        assert_eq!(
            identifiers, expected_identifiers,
            "case {case:?}: expected the declared identifiers in order, got {batch}"
        );

        for ((identifier, arn), (_, depends_on)) in groups.iter().zip(expected.iter()) {
            let case = format!("{case}, group {identifier:?}");
            assert!(
                arn.starts_with(&format!("arn:aws:codebuild:{REGION}:")) && arn.contains(":build/"),
                "case {case:?}: the group must name its build by ARN, got {arn:?}"
            );
            // The ARN resolves to a real build: the batch reports nothing it did not
            // start, and the ARN it publishes is an identifier `BatchGetBuilds` takes.
            let (status, builds) = call(&router, "BatchGetBuilds", json!({"ids": [arn]})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: BatchGetBuilds expected 200, got {status} with {builds}"
            );
            let build = builds["builds"][0].clone();
            assert_eq!(
                build["arn"],
                json!(arn),
                "case {case:?}: the build the group names must be retrievable, got {builds}"
            );
            assert_eq!(
                build["projectName"],
                json!(project),
                "case {case:?}: a build of the batch belongs to the batch's project, got {build}"
            );
            assert_eq!(
                build["buildBatchArn"],
                json!(batch_arn),
                "case {case:?}: a build of the batch names the batch it belongs to, got {build}"
            );
            assert!(
                build["buildNumber"].is_number(),
                "case {case:?}: a build of the batch takes a build number of its project, \
                 got {build}"
            );
            // The dependencies the batchspec declared are reported back as the group's.
            let reported: Vec<String> = batch["buildGroups"]
                .as_array()
                .and_then(|groups| {
                    groups
                        .iter()
                        .find(|group| group["identifier"] == json!(identifier))
                })
                .and_then(|group| group["dependsOn"].as_array())
                .map(|listed| {
                    listed
                        .iter()
                        .map(|entry| entry.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                reported,
                depends_on
                    .iter()
                    .map(|id| (*id).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: the group must report the dependencies it declared, got {batch}"
            );
        }

        // Every build of the batch is a build of the project, so the project's own
        // listing reports them all.
        let (_, listed) = call(
            &router,
            "ListBuildsForProject",
            json!({"projectName": project}),
        )
        .await;
        let listed_count = listed["ids"].as_array().map_or(0, Vec::len);
        assert_eq!(
            listed_count,
            groups.len(),
            "case {case:?}: the project must list the builds the batch started, got {listed}"
        );

        // And the batch itself is readable back with the same groups, from the store.
        let (_, fetched) = call(
            &router,
            "BatchGetBuildBatches",
            json!({"ids": [batch["id"].as_str().unwrap_or_default()]}),
        )
        .await;
        assert_eq!(
            reported_groups(&fetched["buildBatches"][0]),
            groups,
            "case {case:?}: the stored batch must report the same groups, got {fetched}"
        );
    }
}

/// RULE 1, at the ceiling: the builds a batchspec declares are records this store keeps,
/// so the count is a growth site with a ceiling — a batchspec declaring more builds than
/// the ceiling is REFUSED rather than fanned out into a short group list, and nothing is
/// started. The rows walk the boundary: the ceiling exactly, one past it, and far past it.
#[tokio::test]
async fn a_batchspec_declaring_more_builds_than_the_ceiling_is_refused() {
    let ceiling = state::MAX_BATCH_BUILD_GROUPS;
    // (case, how many identifiers the batchspec declares, whether the start is admitted)
    let rows: Vec<(&str, usize, bool)> = vec![
        ("one build", 1, true),
        ("the ceiling exactly", ceiling, true),
        ("one past the ceiling", ceiling + 1, false),
        ("far past the ceiling", ceiling * 3, false),
    ];

    for (index, (case, declared, accepted)) in rows.into_iter().enumerate() {
        let router = emulator();
        let project = format!("fan-out-ceiling-{index}");
        let identifiers: Vec<String> = (0..declared).map(|entry| format!("build{entry}")).collect();
        let named: Vec<&str> = identifiers.iter().map(String::as_str).collect();
        create_project(
            &router,
            &project,
            merged(source_with(&build_list_spec(&named)), batch_enabled()),
        )
        .await;

        let (status, body) = call(
            &router,
            "StartBuildBatch",
            json!({"projectName": project}),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
            assert_eq!(
                reported_groups(&body["buildBatch"]).len(),
                declared + 1,
                "case {case:?}: every declared build must be published, beside the \
                 batch's own source download, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                body["__type"], json!("InvalidInputException"),
                "case {case:?}: expected InvalidInputException, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(state::too_many_batch_build_groups_message()),
                "case {case:?}: expected the ceiling sentence, got {body}"
            );
            // EFFECT ABSENT: a refused start fans out into nothing at all.
            let (_, batches) = call(
                &router,
                "ListBuildBatchesForProject",
                json!({"projectName": project}),
            )
            .await;
            assert_eq!(
                batches["ids"].as_array().map_or(0, Vec::len),
                0,
                "case {case:?}: a refused start must leave no batch build, got {batches}"
            );
            let (_, builds) = call(
                &router,
                "ListBuildsForProject",
                json!({"projectName": project}),
            )
            .await;
            assert_eq!(
                builds["ids"].as_array().map_or(0, Vec::len),
                0,
                "case {case:?}: a refused start must leave no build, got {builds}"
            );
        }
    }
}

/// RULE 2: `BatchGetBuildBatches` names every identifier it could not resolve by the
/// build-batch ARN of that identifier, beside the batches it did resolve.
#[tokio::test]
async fn batch_get_build_batches_names_what_it_could_not_resolve_by_arn() {
    let router = emulator();
    create_project(&router, "not-found-batches", batch_enabled()).await;
    let held = start_build_batch(&router, json!({"projectName": "not-found-batches"})).await;
    let held_id = held["id"].as_str().unwrap_or_default().to_string();
    let held_arn = held["arn"].as_str().unwrap_or_default().to_string();

    // (case, the identifier asked for, the entry expected in buildBatchesNotFound). A
    // bare identifier is reported as the ARN this service forms for it; one already
    // given as an ARN is reported back as sent.
    let absent_id = "not-found-batches:00000000-0000-8000-8000-000000000000";
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a batch id nothing started",
            absent_id.to_string(),
            state::build_batch_arn(REGION, absent_id),
        ),
        (
            "a batch id of a project that does not exist",
            "no-such-project:0001".to_string(),
            state::build_batch_arn(REGION, "no-such-project:0001"),
        ),
        (
            "the held batch's id in the wrong case",
            held_id.to_uppercase(),
            state::build_batch_arn(REGION, &held_id.to_uppercase()),
        ),
        (
            "the ARN of a batch nothing started",
            state::build_batch_arn(REGION, absent_id),
            state::build_batch_arn(REGION, absent_id),
        ),
        (
            "a build ARN, which is not a batch build",
            state::build_arn(REGION, absent_id),
            state::build_arn(REGION, absent_id),
        ),
    ];

    for (case, asked_for, expected) in rows {
        // Asked beside the batch this account DOES hold, so each row proves the halves
        // are partitioned rather than that the whole answer was empty.
        let (status, body) = call(
            &router,
            "BatchGetBuildBatches",
            json!({"ids": [held_id.clone(), asked_for.clone()]}),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {body}");
        assert_eq!(
            body["buildBatches"][0]["arn"],
            json!(held_arn),
            "case {case:?}: the held batch must still resolve, got {body}"
        );
        assert_eq!(
            body["buildBatchesNotFound"],
            json!([expected]),
            "case {case:?}: expected the identifier named by ARN, got {body}"
        );
    }

    // Nothing is reported missing when every identifier resolved.
    let (_, body) = call(
        &router,
        "BatchGetBuildBatches",
        json!({"ids": [held_id.clone()]}),
    )
    .await;
    assert_eq!(
        body["buildBatchesNotFound"],
        json!([]),
        "every identifier resolved, so the not-found half must be [], got {body}"
    );
}
