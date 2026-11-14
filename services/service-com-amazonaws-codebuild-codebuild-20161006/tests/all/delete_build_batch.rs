// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteBuildBatch`, driven over the wire the router serves
//! (`POST /` with `X-Amz-Target: CodeBuild_20161006.DeleteBuildBatch` on awsJson1_1)
//! through the harness in `project_build_lifecycle`.
//!
//! RULE under test: `DeleteBuildBatch` removes a batch build only once that batch has
//! finished — a batch still running is refused with the single error the operation
//! models, named by its full ARN in the sentence CodeBuild words for that refusal, and
//! stays exactly where it was — and the delete removes the builds that
//! batch is made of with it, reporting their ids in `buildsDeleted`, the ids it held
//! no build record for in `buildsNotDeleted` with a `statusCode` saying so, and the
//! batch-level outcome in `statusCode`. A removed batch is gone from every later read,
//! while an id that denotes no batch build of this account deletes nothing and is
//! refused with that same modelled error.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::{
    Build, BuildBatch, BuildGroup, BuildSummary,
};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredBuild,
    StoredBuildBatch,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::project_build_lifecycle::{
    batch_enabled, call, create_project, emulator, emulator_on, error_type, expected_arn,
    listed_ids, PAST_THE_SETTLE_SPAN, REGION,
};

/// Start one batch build of `project` and hand back its id.
async fn start_build_batch(router: &MultiServiceRouter, project: &str) -> String {
    let (status, body) = call(router, "StartBuildBatch", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartBuildBatch {project:?}: expected 200, got {status} with {body}"
    );
    body["buildBatch"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("StartBuildBatch {project:?} must report an id, got {body}"))
        .to_string()
}

/// A syntactically valid batch build id (`<projectName>:<uuid>`) that names no batch
/// build: every id this service mints derives its uuid from the project, the batch
/// number and the start instant, so this one is never minted.
fn unknown_batch_id(project: &str, nth: usize) -> String {
    format!("{project}:00000000-0000-8000-8000-{nth:012}")
}

/// The entries of a `*NotFound` / `*Deleted` list of ids, as strings.
fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Which batch build the delete call names, and so what it must answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// A batch build whose clock-owed transition has fallen due: it is complete.
    Finished,
    /// A batch build still inside its settle span: it is not complete.
    Running,
    /// A well-formed id naming no batch build of a project that does exist.
    AbsentOfKnownProject,
    /// A well-formed id whose project half names no project either.
    AbsentOfUnknownProject,
    /// A finished batch build deleted twice — the second call is the one asserted.
    Repeated,
}

/// RULE: a finished batch build is deleted and answered with the batch-level
/// `statusCode`; one still running is refused with the modelled
/// `InvalidInputException` and left in place; an id that denotes no batch build of this
/// account is refused with that same error and deletes nothing. Every row reads the
/// result back through `BatchGetBuildBatches` and `ListBuildBatches`, which are computed
/// from the store the delete wrote.
#[tokio::test]
async fn delete_build_batch_removes_a_finished_batch_and_refuses_one_still_running() {
    let cases: &[(Target, &str)] = &[
        (Target::Finished, "del-batch-finished"),
        (Target::Running, "del-batch-running"),
        (Target::AbsentOfKnownProject, "del-batch-absent"),
        (Target::AbsentOfUnknownProject, "del-batch-absent-project"),
        (Target::Repeated, "del-batch-repeat"),
    ];

    for (target, project) in cases {
        // A fresh emulator per row: a row's state must not depend on the one before.
        let router = emulator();
        create_project(&router, project, batch_enabled()).await;
        // Two batch builds, so every row also proves the one the call did not name
        // survives it.
        let first = start_build_batch(&router, project).await;
        let second = start_build_batch(&router, project).await;

        if matches!(target, Target::Finished | Target::Repeated) {
            tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
        }

        let requested = match target {
            Target::Finished | Target::Running | Target::Repeated => first.clone(),
            Target::AbsentOfKnownProject => unknown_batch_id(project, 1),
            Target::AbsentOfUnknownProject => unknown_batch_id("del-batch-never-created", 2),
        };

        if *target == Target::Repeated {
            let (status, body) =
                call(&router, "DeleteBuildBatch", json!({"id": requested.clone()})).await;
            assert_eq!(
                status, 200,
                "case {target:?}: the first delete: expected 200, got {status} with {body}"
            );
        }

        let (status, body) =
            call(&router, "DeleteBuildBatch", json!({"id": requested.clone()})).await;

        match target {
            Target::Finished => {
                assert_eq!(
                    status, 200,
                    "case {target:?}: expected 200, got {status} with {body}"
                );
                assert_eq!(
                    body["statusCode"],
                    Value::String(state::BATCH_DELETED_STATUS_CODE.to_string()),
                    "case {target:?}: a deleted batch is answered with the batch-level \
                     statusCode, got {body}"
                );
                // Nothing here reads a batch build specification, so the batch started
                // no build of its own and the partition is empty on both sides. The
                // builds-of-a-batch half of the rule is covered by the seeded suite
                // below.
                assert_eq!(
                    body["buildsDeleted"],
                    json!([]),
                    "case {target:?}: the deleted batch had no build of its own, got {body}"
                );
                assert_eq!(
                    body["buildsNotDeleted"],
                    json!([]),
                    "case {target:?}: nothing was left behind, so buildsNotDeleted is \
                     an empty list, got {body}"
                );
            }
            Target::Running => {
                assert_eq!(
                    status, 400,
                    "case {target:?}: expected 400, got {status} with {body}"
                );
                assert_eq!(
                    error_type(&body),
                    "InvalidInputException",
                    "case {target:?}: expected InvalidInputException, got {body}"
                );
                assert_eq!(
                    body["message"],
                    Value::String(format!(
                        "Cannot delete BuildBatch {}, as it is still in progress",
                        state::build_batch_arn(REGION, &requested)
                    )),
                    "case {target:?}: the refusal names the batch it would not delete by its \
                     full ARN, got {body}"
                );
            }
            Target::AbsentOfKnownProject
            | Target::AbsentOfUnknownProject
            | Target::Repeated => {
                assert_eq!(
                    status, 400,
                    "case {target:?}: expected 400, got {status} with {body}"
                );
                assert_eq!(
                    error_type(&body),
                    "InvalidInputException",
                    "case {target:?}: expected InvalidInputException, got {body}"
                );
                assert_eq!(
                    body["message"],
                    Value::String(state::INVALID_BUILD_BATCH_ID_MESSAGE.to_string()),
                    "case {target:?}: an id denoting no batch build is refused in \
                     CodeBuild's own words, got {body}"
                );
                assert!(
                    body["statusCode"].is_null()
                        && body["buildsDeleted"].is_null()
                        && body["buildsNotDeleted"].is_null(),
                    "case {target:?}: a refusal reports no delete outcome at all, got {body}"
                );
            }
        }

        // What every later read sees is the store the call wrote, not a per-call answer.
        let gone = matches!(target, Target::Finished | Target::Repeated);
        let (status, after) = call(
            &router,
            "BatchGetBuildBatches",
            json!({"ids": [first.clone(), second.clone()]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {target:?}: BatchGetBuildBatches after the delete: expected 200, got \
             {status} with {after}"
        );
        let reported: Vec<String> = after["buildBatches"]
            .as_array()
            .map(|batches| {
                batches
                    .iter()
                    .map(|batch| batch["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let not_found = string_list(&after["buildBatchesNotFound"]);
        if gone {
            // A batch nothing resolved is named in the ARN form the service names a
            // batch by everywhere else, not by the bare id the request carried.
            let first_arn = state::build_batch_arn(REGION, &first);
            assert!(
                !reported.contains(&first) && not_found.contains(&first_arn),
                "case {target:?}: the deleted batch reads as not found by its ARN, got {after}"
            );
        } else {
            assert!(
                reported.contains(&first),
                "case {target:?}: no batch was deleted, so {first} is still readable, \
                 got {after}"
            );
        }
        assert!(
            reported.contains(&second),
            "case {target:?}: the batch the call did not name must still be readable, \
             got {after}"
        );

        let (status, listed) = call(&router, "ListBuildBatches", json!({})).await;
        assert_eq!(
            status, 200,
            "case {target:?}: ListBuildBatches: expected 200, got {status} with {listed}"
        );
        let listed_ids = listed_ids(&listed);
        assert_eq!(
            listed_ids.contains(&first),
            !gone,
            "case {target:?}: the account listing must report {first} only while it is \
             held, got {listed}"
        );
        assert!(
            listed_ids.contains(&second),
            "case {target:?}: the batch the call did not name stays listed, got {listed}"
        );
    }
}

/// RULE: a deleted batch build frees nothing a later batch build reuses — AWS
/// documents that "if a batch build is deleted, the `buildBatchNumber` of other batch
/// builds does not change" — so the project's sequence keeps counting up.
#[tokio::test]
async fn deleting_a_batch_build_does_not_rewind_the_batch_number() {
    let router = emulator();
    create_project(&router, "del-batch-numbers", batch_enabled()).await;

    let mut ids = Vec::new();
    for expected in 1..=3_i64 {
        let (status, body) = call(
            &router,
            "StartBuildBatch",
            json!({"projectName": "del-batch-numbers"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "batch {expected}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["buildBatch"]["buildBatchNumber"].as_i64(),
            Some(expected),
            "batch {expected}: expected buildBatchNumber {expected}, got {body}"
        );
        ids.push(
            body["buildBatch"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;

    for id in &ids {
        let (status, body) = call(&router, "DeleteBuildBatch", json!({"id": id})).await;
        assert_eq!(
            status, 200,
            "DeleteBuildBatch {id}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["statusCode"],
            Value::String(state::BATCH_DELETED_STATUS_CODE.to_string()),
            "DeleteBuildBatch {id}: expected the batch-level statusCode, got {body}"
        );
    }

    let (status, body) = call(
        &router,
        "StartBuildBatch",
        json!({"projectName": "del-batch-numbers"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the batch build started after the deletes: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["buildBatch"]["buildBatchNumber"].as_i64(),
        Some(4),
        "a deleted batch build must not free its buildBatchNumber, got {body}"
    );
}

/// What one `BuildSummary$arn` of a seeded batch build names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// A build ARN whose build this account holds a record of, by index.
    HeldBuild(usize),
    /// A build ARN this account holds no build record for, by index.
    AbsentBuild(usize),
    /// A well-formed ARN of this service that is not a build ARN (the project's own),
    /// so no build id can be resolved from it.
    NotABuildArn,
}

/// One row: the build groups the stored batch build carries, each as its current
/// build summary and the prior attempts retained beside it.
#[derive(Debug)]
struct Seeded {
    name: &'static str,
    project: &'static str,
    groups: &'static [(Option<Named>, &'static [Named])],
}

/// How many builds each seeded row holds in the build store.
const HELD_BUILDS: usize = 3;

/// The id of the `index`th build seeded into `project`'s build records.
fn held_id(project: &str, index: usize) -> String {
    format!("{project}:00000000-0000-8000-8000-{index:012}")
}

/// The id of the `index`th build no record is held for.
fn absent_id(project: &str, index: usize) -> String {
    format!("{project}:ffffffff-0000-8000-8000-{index:012}")
}

/// The `arn` a summary naming `named` carries.
fn summary_arn(named: Named, project: &str) -> String {
    match named {
        Named::HeldBuild(index) => state::build_arn(REGION, &held_id(project, index)),
        Named::AbsentBuild(index) => state::build_arn(REGION, &absent_id(project, index)),
        Named::NotABuildArn => state::project_arn(REGION, project),
    }
}

/// The build id a summary naming `named` resolves to, if any.
fn summary_build_id(named: Named, project: &str) -> Option<String> {
    match named {
        Named::HeldBuild(index) => Some(held_id(project, index)),
        Named::AbsentBuild(index) => Some(absent_id(project, index)),
        Named::NotABuildArn => None,
    }
}

/// `row`'s groups as the modelled [`BuildGroup`] list a stored batch build carries.
fn build_groups(row: &Seeded) -> Vec<BuildGroup> {
    row.groups
        .iter()
        .enumerate()
        .map(|(index, (current, prior))| BuildGroup {
            identifier: Some(format!("group-{index}")),
            current_build_summary: current.map(|named| BuildSummary {
                arn: Some(summary_arn(named, row.project)),
                ..BuildSummary::default()
            }),
            prior_build_summary_list: Some(
                prior
                    .iter()
                    .map(|named| BuildSummary {
                        arn: Some(summary_arn(*named, row.project)),
                        ..BuildSummary::default()
                    })
                    .collect(),
            ),
            ..BuildGroup::default()
        })
        .collect()
}

/// Every build id `row`'s groups name, in the order the groups name them, each once.
fn named_build_ids(row: &Seeded) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for (current, prior) in row.groups {
        for named in current.iter().chain(prior.iter()) {
            if let Some(id) = summary_build_id(*named, row.project) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

/// RULE: deleting a batch build deletes the builds that batch is made of — the builds
/// its groups name, whether by their current summary or a prior attempt — reporting
/// each named build once, in `buildsDeleted` when a record was held and removed and in
/// `buildsNotDeleted` with its id and a `statusCode` saying why when none was held.
/// The table varies the number of groups, the presence of prior attempts, held against
/// unheld builds, a build named twice and an ARN that is not a build ARN at all.
#[tokio::test]
async fn the_builds_of_a_deleted_batch_build_are_deleted_with_it() {
    let rows: &[Seeded] = &[
        Seeded {
            name: "one group naming one held build",
            project: "del-batch-one",
            groups: &[(Some(Named::HeldBuild(0)), &[])],
        },
        Seeded {
            name: "two groups, one carrying a prior attempt",
            project: "del-batch-groups",
            groups: &[
                (Some(Named::HeldBuild(0)), &[Named::HeldBuild(1)]),
                (Some(Named::HeldBuild(2)), &[]),
            ],
        },
        Seeded {
            name: "a group naming a build no record is held for",
            project: "del-batch-unheld",
            groups: &[
                (Some(Named::AbsentBuild(0)), &[]),
                (Some(Named::HeldBuild(1)), &[]),
            ],
        },
        Seeded {
            name: "the same build named by two summaries",
            project: "del-batch-repeat-build",
            groups: &[
                (Some(Named::HeldBuild(0)), &[Named::HeldBuild(0)]),
                (Some(Named::HeldBuild(0)), &[]),
            ],
        },
        Seeded {
            name: "a summary whose arn is not a build arn",
            project: "del-batch-other-arn",
            groups: &[
                (Some(Named::NotABuildArn), &[]),
                (Some(Named::HeldBuild(2)), &[]),
            ],
        },
        Seeded {
            name: "a group with no summary at all",
            project: "del-batch-no-summary",
            groups: &[(None, &[])],
        },
        Seeded {
            name: "no build group at all",
            project: "del-batch-no-groups",
            groups: &[],
        },
    ];

    for row in rows {
        let batch_id = format!("{}:batch-0000-8000-8000-000000000001", row.project);
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // Seeded through the very store the handlers write, because nothing in this
        // emulator reads a batch build specification: a batch it STARTS carries no
        // build group, so the groups a real batch build carries are seeded here.
        {
            let handle = engine.state::<CodeBuildState>();
            let mut guard = handle.lock();
            let started = state::now_millis() - 10_000;
            for index in 0..HELD_BUILDS {
                let id = held_id(row.project, index);
                guard.insert_build(
                    id.clone(),
                    StoredBuild {
                        build: Build {
                            id: Some(id),
                            project_name: Some(row.project.to_string()),
                            build_complete: true,
                            ..Build::default()
                        },
                        start_millis: started,
                        settle_due_millis: started,
                        artifacts: None,
                        secondary_artifacts: Vec::new(),
                        run_claimed: false,
                    },
                );
            }
            guard.insert_build_batch(
                batch_id.clone(),
                StoredBuildBatch {
                    build_batch: BuildBatch {
                        id: Some(batch_id.clone()),
                        project_name: Some(row.project.to_string()),
                        complete: true,
                        build_groups: Some(build_groups(row)),
                        ..BuildBatch::default()
                    },
                    start_millis: started,
                    settle_due_millis: started,
                },
            );
        }
        let router = emulator_on(Arc::clone(&engine));

        let named = named_build_ids(row);
        let expected_deleted: Vec<String> = named
            .iter()
            .filter(|id| (0..HELD_BUILDS).any(|index| &held_id(row.project, index) == *id))
            .cloned()
            .collect();
        let expected_not_deleted: Vec<String> = named
            .iter()
            .filter(|id| !expected_deleted.contains(id))
            .cloned()
            .collect();

        let (status, body) = call(&router, "DeleteBuildBatch", json!({"id": batch_id.clone()})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {body}",
            row.name
        );
        assert_eq!(
            body["statusCode"],
            Value::String(state::BATCH_DELETED_STATUS_CODE.to_string()),
            "case {:?}: a deleted batch is answered with the batch-level statusCode, got {body}",
            row.name
        );
        assert_eq!(
            string_list(&body["buildsDeleted"]),
            expected_deleted,
            "case {:?}: buildsDeleted must report each held build of the batch once, in \
             build-group order; got {body}",
            row.name
        );
        let not_deleted: Vec<String> = body["buildsNotDeleted"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            not_deleted, expected_not_deleted,
            "case {:?}: buildsNotDeleted must report each named build no record was held \
             for; got {body}",
            row.name
        );
        for (position, id) in expected_not_deleted.iter().enumerate() {
            assert_eq!(
                body["buildsNotDeleted"][position]["statusCode"],
                Value::String(format!("Build {id} does not exist")),
                "case {:?}: the statusCode for {id} must name that id and say why; got {body}",
                row.name
            );
        }

        // The builds really left the build records, and the ones the batch did not
        // name are untouched.
        let all_held: Vec<String> = (0..HELD_BUILDS).map(|i| held_id(row.project, i)).collect();
        let (status, builds) = call(&router, "BatchGetBuilds", json!({"ids": all_held.clone()})).await;
        assert_eq!(
            status, 200,
            "case {:?}: BatchGetBuilds after the delete: expected 200, got {status} with {builds}",
            row.name
        );
        let reported: Vec<String> = builds["builds"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|build| build["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        for id in &all_held {
            let survives = !expected_deleted.contains(id);
            assert_eq!(
                reported.contains(id),
                survives,
                "case {:?}: {id} must {} be readable after the delete; got {builds}",
                row.name,
                if survives { "still" } else { "no longer" }
            );
        }

        // And the batch build itself is gone.
        let (status, batches) =
            call(&router, "BatchGetBuildBatches", json!({"ids": [batch_id.clone()]})).await;
        assert_eq!(
            status, 200,
            "case {:?}: BatchGetBuildBatches after the delete: expected 200, got {status} \
             with {batches}",
            row.name
        );
        assert_eq!(
            string_list(&batches["buildBatchesNotFound"]),
            vec![state::build_batch_arn(REGION, &batch_id)],
            "case {:?}: the deleted batch build reads as not found by its ARN; got {batches}",
            row.name
        );
    }
}

/// Whether the stored batch build carries an ARN of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoredArn {
    /// It carries the ARN this service forms for its id — what a batch build
    /// `StartBuildBatch` stored holds.
    Held,
    /// It carries none, so the refusal forms the ARN for that id in the region the
    /// call was signed for.
    Absent,
}

/// One row of the in-progress refusal table: the halves of the batch build id, and
/// what the stored record holds for its ARN.
#[derive(Debug)]
struct Running {
    name: &'static str,
    /// The project half of the batch build id (`<projectName>:<uuid>`).
    project: &'static str,
    /// The uuid half.
    uuid: &'static str,
    stored_arn: StoredArn,
}

/// RULE: refusing to act on a resource, CodeBuild names that resource by its full
/// ARN — `arn:aws:codebuild:<region>:<account>:build-batch/<batch-id>` for a batch
/// build — never by the bare id, so the refusal of a batch build that has not
/// finished reads `Cannot delete BuildBatch <arn>, as it is still in progress`.
///
/// The table varies both halves of the id and whether the stored record carries its
/// own ARN, so the message's one variable part is exercised across the class rather
/// than on a single id; every row also proves the refused batch is left intact and
/// still running.
#[tokio::test]
async fn refusing_a_batch_build_that_has_not_finished_names_it_by_its_arn() {
    let rows: &[Running] = &[
        Running {
            name: "a record carrying the ARN this service formed for it",
            project: "del-batch-msg",
            uuid: "7aa4009f-8169-8000-8000-000000000001",
            stored_arn: StoredArn::Held,
        },
        Running {
            name: "a record carrying no ARN of its own",
            project: "del-batch-msg",
            uuid: "7aa4009f-8169-8000-8000-000000000002",
            stored_arn: StoredArn::Absent,
        },
        Running {
            name: "the shortest project name a batch build id can carry",
            project: "p1",
            uuid: "00000000-0000-8000-8000-000000000003",
            stored_arn: StoredArn::Absent,
        },
        Running {
            name: "a long project name carrying digits, hyphens and an underscore",
            project: "sim2real-cb_batch-project-0123456789-abcdefghij",
            uuid: "c2600cc7-f412-8aee-b311-7f2989efd1f6",
            stored_arn: StoredArn::Held,
        },
    ];

    for row in rows {
        let id = format!("{}:{}", row.project, row.uuid);
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // Seeded through the store the handlers write, so the row controls exactly
        // what the record holds: a batch build whose settle instant is far ahead is
        // one the clock owes nothing, which is what "has not finished" means here.
        {
            let handle = engine.state::<CodeBuildState>();
            let mut guard = handle.lock();
            let started = state::now_millis();
            guard.insert_build_batch(
                id.clone(),
                StoredBuildBatch {
                    build_batch: BuildBatch {
                        id: Some(id.clone()),
                        arn: match row.stored_arn {
                            StoredArn::Held => Some(state::build_batch_arn(REGION, &id)),
                            StoredArn::Absent => None,
                        },
                        project_name: Some(row.project.to_string()),
                        build_batch_status: Some("IN_PROGRESS".to_string()),
                        complete: false,
                        ..BuildBatch::default()
                    },
                    start_millis: started,
                    settle_due_millis: started + 60_000,
                },
            );
        }
        let router = emulator_on(Arc::clone(&engine));

        let (status, body) = call(&router, "DeleteBuildBatch", json!({"id": id.clone()})).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} with {body}",
            row.name
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {:?}: expected InvalidInputException, got {body}",
            row.name
        );
        // Built here from the region the call is signed for and the emulator's
        // account, independently of the handler, so the assertion pins the ARN the
        // message must carry rather than echoing whatever it produced.
        let arn = expected_arn(&format!("build-batch/{id}"));
        assert_eq!(
            body["message"],
            Value::String(format!(
                "Cannot delete BuildBatch {arn}, as it is still in progress"
            )),
            "case {:?}: the refusal must name the batch build by its full ARN, got {body}",
            row.name
        );

        // The batch is left exactly where it was: still held, still running.
        let (status, after) =
            call(&router, "BatchGetBuildBatches", json!({"ids": [id.clone()]})).await;
        assert_eq!(
            status, 200,
            "case {:?}: BatchGetBuildBatches after the refusal: expected 200, got {status} \
             with {after}",
            row.name
        );
        assert_eq!(
            after["buildBatches"][0]["id"],
            Value::String(id.clone()),
            "case {:?}: the refused batch build is still readable, got {after}",
            row.name
        );
        assert_eq!(
            after["buildBatches"][0]["buildBatchStatus"],
            Value::String("IN_PROGRESS".to_string()),
            "case {:?}: the refused batch build is still running, got {after}",
            row.name
        );
        assert_eq!(
            after["buildBatches"][0]["complete"],
            Value::Bool(false),
            "case {:?}: the refused batch build has not been completed by the refusal, \
             got {after}",
            row.name
        );
    }
}
