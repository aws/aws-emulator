// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SENTENCE each not-found refusal of the Aurora Limitless shard
//! group, snapshot-export and blue/green families carries.
//!
//! The rule under test: a not-found refusal answers the sentence Amazon RDS words for that
//! operation's fault — its fixed text exactly, and only the variable parts the service puts
//! in it, taken from the request — rather than one house-style template built from the
//! resource type and the identifier the request carried. Concretely, over the operations
//! whose recorded refusals this suite spans:
//!
//! * `ModifyDBShardGroup` answers `DBShardGroupNotFound` (404, the `awsQueryError` code of
//!   `DBShardGroupNotFoundFault` —
//!   `smithy_query(query="operation", operation="ModifyDBShardGroup,DeleteDBShardGroup,
//!   RebootDBShardGroup")`) with one FIXED sentence about the identifier-and-account pair,
//!   while `DeleteDBShardGroup` and `RebootDBShardGroup` answer the same fault with the
//!   FIXED sentence about a shard group that cannot be reached. Neither carries the
//!   identifier.
//! * `CreateDBShardGroup` answers `DBClusterNotFoundFault` (404) for a primary cluster it
//!   cannot resolve with one FIXED sentence that names no cluster.
//! * `CancelExportTask` answers `ExportTaskNotFound` (404) with a sentence that NAMES the
//!   `ExportTaskIdentifier` the request carried, unquoted, in the spelling it was sent in.
//! * `DeleteBlueGreenDeployment` and `DescribeBlueGreenDeployments` answer
//!   `BlueGreenDeploymentNotFoundFault` (404) with a sentence that names the deployment
//!   identifier the request carried, SINGLE-QUOTED.
//!
//! Each table spans what the rule generalises over: the shape of an identifier nothing
//! holds (a fresh name, a near miss of a stored one, a single letter, mixed case, a name at
//! the published length bound), how it came to hold nothing (never created, created and
//! then deleted, held only by another region), and — for the sentences with a variable part
//! — a value the test never spells out, taken from what the service minted.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so each
//! assertion reads the XML the emulator itself answered with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, text, wait_for_settle, REGION,
};
use crate::export_tasks::{emulator_holding, spec, stored_task};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second region, so a resource that exists but is out of the signed region's scope is a
/// row of each table.
const OTHER_REGION: &str = "ap-south-1";

/// The engine the primary cluster of every shard group below runs: Aurora Limitless
/// Database is an Aurora PostgreSQL feature.
const CLUSTER_ENGINE: &str = "aurora-postgresql";

/// `MaxACU`, which `CreateDBShardGroup` requires.
const MAX_ACU: &str = "100";

/// The sentence `ModifyDBShardGroup` answers for a shard group it cannot resolve.
const MODIFY_SHARD_GROUP_SENTENCE: &str =
    "The DB shard group with requested identifier in requested account doesn't exist.";

/// The sentence `DeleteDBShardGroup` and `RebootDBShardGroup` answer for a shard group they
/// cannot resolve. It carries no terminal period.
const SHARD_GROUP_UNREACHABLE_SENTENCE: &str = "DB shard group not found or can not be accessed";

/// The sentence `CreateDBShardGroup` answers for a primary DB cluster it cannot resolve.
const CLUSTER_DOES_NOT_EXIST_SENTENCE: &str = "The DB cluster doesn't exist.";

/// The longest `DBShardGroupIdentifier` the member admits: `length 1..63` on the modelled
/// `DBShardGroupIdentifier` shape
/// (`smithy_query(query="shape", shape="DBShardGroupIdentifier")`), whose pattern
/// `^[a-zA-Z](?:-?[a-zA-Z0-9]+)*$` the generated name also satisfies.
const MAX_SHARD_GROUP_IDENTIFIER_LEN: usize = 63;

/// The sentence a blue/green not-found refusal carries for `identifier`.
fn blue_green_sentence(identifier: &str) -> String {
    format!("Blue Green Deployment '{identifier}' doesn't exist.")
}

/// The sentence `CancelExportTask` carries for `identifier`.
fn export_task_sentence(identifier: &str) -> String {
    format!("The export task {identifier} doesn't exist.")
}

/// The number one `<tag>` element carries, as an `f64` — `MaxACU` is a double, so an
/// assertion is on the value and not on how it was rendered.
fn number(case: &str, xml: &str, tag: &str) -> f64 {
    let raw = text(xml, tag);
    raw.parse::<f64>()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a number in <{tag}>, got {raw:?}"))
}

/// An identifier of exactly [`MAX_SHARD_GROUP_IDENTIFIER_LEN`] characters that nothing
/// created, so the published bound is one of the rows.
fn identifier_at_the_published_bound() -> String {
    let mut identifier = String::from("e");
    identifier.extend(std::iter::repeat_n('a', MAX_SHARD_GROUP_IDENTIFIER_LEN - 1));
    assert_eq!(
        identifier.chars().count(),
        MAX_SHARD_GROUP_IDENTIFIER_LEN,
        "the row at the published bound must be {MAX_SHARD_GROUP_IDENTIFIER_LEN} characters, \
         got {}",
        identifier.chars().count()
    );
    identifier
}

/// `CreateDBCluster` in `region`.
///
/// `ClusterScalabilityType` is `limitless` and the engine version is the one Aurora Limitless
/// Database is offered on: a DB shard group holds the shards of a Limitless database, so only
/// a cluster created that way can take one.
async fn create_cluster(router: &MultiServiceRouter, region: &str, identifier: &str) {
    let case = format!("create the primary cluster {identifier} in {region}");
    let engine_version =
        crate::instance_reference_resolution_refusals::limitless_engine_version(router).await;
    expect_ok(
        &case,
        call_in(
            router,
            region,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", identifier.to_string()),
                ("Engine", CLUSTER_ENGINE.to_string()),
                ("ClusterScalabilityType", "limitless".to_string()),
                ("EngineVersion", engine_version),
            ],
        )
        .await,
    );
}

/// `CreateDBShardGroup` in `region`, against `cluster`.
async fn create_shard_group(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    cluster: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateDBShardGroup",
        &[
            ("DBShardGroupIdentifier", identifier),
            ("DBClusterIdentifier", cluster),
            ("MaxACU", MAX_ACU),
        ],
    )
    .await
}

/// One `DBShardGroupIdentifier` the acting shard-group calls cannot resolve in [`REGION`].
struct MissingShardGroup {
    case: &'static str,
    identifier: String,
}

#[tokio::test]
async fn the_acting_shard_group_calls_each_word_the_one_not_found_fault_their_own_way() {
    let router = emulator();
    create_cluster(&router, REGION, "emu-sentence-cluster").await;
    create_cluster(&router, OTHER_REGION, "emu-sentence-cluster").await;

    // One shard group this region holds, so a near miss of its identifier is a row and no
    // refusal below can pass merely because the store is empty; one only the OTHER region
    // holds; and one this region held until its delete ran.
    expect_ok(
        "the stored shard group",
        create_shard_group(
            &router,
            REGION,
            "emu-sentence-stored",
            "emu-sentence-cluster",
        )
        .await,
    );
    expect_ok(
        "the other region's shard group",
        create_shard_group(
            &router,
            OTHER_REGION,
            "emu-sentence-elsewhere",
            "emu-sentence-cluster",
        )
        .await,
    );
    expect_ok(
        "the shard group that is deleted below",
        create_shard_group(
            &router,
            REGION,
            "emu-sentence-deleted",
            "emu-sentence-cluster",
        )
        .await,
    );
    wait_for_settle();
    expect_ok(
        "the delete of a shard group that exists",
        call(
            &router,
            "DeleteDBShardGroup",
            &[("DBShardGroupIdentifier", "emu-sentence-deleted")],
        )
        .await,
    );
    wait_for_settle();

    let table = [
        MissingShardGroup {
            case: "an identifier nothing was created under",
            identifier: "emu-sentence-ghost".to_string(),
        },
        MissingShardGroup {
            case: "a near miss of a stored identifier",
            identifier: "emu-sentence-stored2".to_string(),
        },
        MissingShardGroup {
            case: "a single letter",
            identifier: "e".to_string(),
        },
        MissingShardGroup {
            case: "mixed case of an identifier nothing holds",
            identifier: "EMU-Sentence-Ghost".to_string(),
        },
        MissingShardGroup {
            case: "an identifier at the published length bound",
            identifier: identifier_at_the_published_bound(),
        },
        MissingShardGroup {
            case: "a shard group only another region holds",
            identifier: "emu-sentence-elsewhere".to_string(),
        },
        MissingShardGroup {
            case: "a shard group whose delete has already run",
            identifier: "emu-sentence-deleted".to_string(),
        },
    ];

    for row in &table {
        let named = [("DBShardGroupIdentifier", row.identifier.as_str())];

        let modified = expect_error(
            row.case,
            call_in(&router, REGION, "ModifyDBShardGroup", &named).await,
            404,
            "DBShardGroupNotFound",
        );
        let modified = text(&modified, "Message");
        assert_eq!(
            modified, MODIFY_SHARD_GROUP_SENTENCE,
            "case {:?}: expected the modification's sentence, got {modified:?}",
            row.case
        );

        for action in ["DeleteDBShardGroup", "RebootDBShardGroup"] {
            let refused = expect_error(
                row.case,
                call_in(&router, REGION, action, &named).await,
                404,
                "DBShardGroupNotFound",
            );
            let refused = text(&refused, "Message");
            assert_eq!(
                refused, SHARD_GROUP_UNREACHABLE_SENTENCE,
                "case {:?}: expected {action}'s sentence, got {refused:?}",
                row.case
            );
        }

        assert_ne!(
            MODIFY_SHARD_GROUP_SENTENCE, SHARD_GROUP_UNREACHABLE_SENTENCE,
            "case {:?}: the two paths must not answer one undifferentiated sentence",
            row.case
        );
    }

    // The refusals resolved nothing and changed nothing: the shard group this region does
    // hold is still served by the same operation that refused every row above, and the
    // report reads the modification back.
    let modified = expect_ok(
        "the stored shard group",
        call(
            &router,
            "ModifyDBShardGroup",
            &[
                ("DBShardGroupIdentifier", "emu-sentence-stored"),
                ("MaxACU", "200"),
            ],
        )
        .await,
    );
    assert_eq!(
        number("the stored shard group", &modified, "MaxACU"),
        200.0,
        "expected the modification of the stored shard group to be served, got {modified}"
    );
    let reported = expect_ok(
        "the stored shard group after the modification",
        call(
            &router,
            "DescribeDBShardGroups",
            &[("DBShardGroupIdentifier", "emu-sentence-stored")],
        )
        .await,
    );
    assert_eq!(
        number(
            "the stored shard group after the modification",
            &reported,
            "MaxACU"
        ),
        200.0,
        "expected the report to read the modification back, got {reported}"
    );
}

#[tokio::test]
async fn a_create_naming_a_primary_cluster_that_does_not_exist_answers_the_fixed_sentence() {
    let router = emulator();
    create_cluster(&router, OTHER_REGION, "emu-sentence-remote").await;
    wait_for_settle();

    let rows: Vec<(&str, String)> = vec![
        (
            "a cluster identifier nothing was created under",
            "emu-sentence-no-cluster".to_string(),
        ),
        (
            "mixed case of an identifier nothing holds",
            "EMU-Sentence-No-Cluster".to_string(),
        ),
        (
            "the ARN of a cluster nothing created",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-sentence-no-cluster"),
        ),
        (
            "a cluster only another region holds",
            "emu-sentence-remote".to_string(),
        ),
        (
            "the ARN of a cluster only another region holds",
            format!("arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:cluster:emu-sentence-remote"),
        ),
    ];

    for (index, (case, cluster)) in rows.iter().enumerate() {
        let shard_group = format!("emu-sentence-create{index}");
        let refused = expect_error(
            case,
            create_shard_group(&router, REGION, &shard_group, cluster).await,
            404,
            "DBClusterNotFoundFault",
        );
        let refused = text(&refused, "Message");
        assert_eq!(
            refused, CLUSTER_DOES_NOT_EXIST_SENTENCE,
            "case {case:?}: expected the fixed sentence, got {refused:?}"
        );

        // A refused create left nothing behind, so the shard group it named is still one
        // nothing created.
        expect_error(
            case,
            call(
                &router,
                "DescribeDBShardGroups",
                &[("DBShardGroupIdentifier", shard_group.as_str())],
            )
            .await,
            404,
            "DBShardGroupNotFound",
        );
    }

    // And a create naming a cluster this region DOES hold is served, so the refusals above
    // are about resolving the cluster rather than about the operation.
    create_cluster(&router, REGION, "emu-sentence-local").await;
    let created = expect_ok(
        "a cluster this region holds",
        create_shard_group(
            &router,
            REGION,
            "emu-sentence-served",
            "emu-sentence-local",
        )
        .await,
    );
    assert_eq!(
        text(&created, "DBClusterIdentifier"),
        "emu-sentence-local",
        "expected the served create to report its primary cluster, got {created}"
    );
}

#[tokio::test]
async fn a_cancellation_naming_no_export_task_names_the_identifier_it_was_sent() {
    // One store holding the seeded tasks of both regions, and one holding none: the
    // sentence is the same in each, and it carries what the REQUEST sent.
    let mut elsewhere = stored_task(OTHER_REGION, spec("exp-in-progress"));
    elsewhere.identifier = "exp-elsewhere".to_string();
    let stocked = emulator_holding(vec![
        (
            REGION.to_string(),
            stored_task(REGION, spec("exp-in-progress")),
        ),
        (OTHER_REGION.to_string(), elsewhere),
    ]);
    let empty = emulator_holding(Vec::new());

    let identifiers = [
        ("an identifier nothing was created under", "exp-no-such-task"),
        ("a near miss of a stored identifier", "exp-in-progress-2"),
        (
            "another case of a stored identifier, which names no task",
            "EXP-IN-PROGRESS",
        ),
        ("an export task only another region holds", "exp-elsewhere"),
        ("a single character", "x"),
        (
            "an identifier carrying the punctuation the member admits",
            "exp.nightly_2026-01-01",
        ),
    ];

    for (case, identifier) in identifiers {
        for (store, router) in [("a stocked store", &stocked), ("an empty store", &empty)] {
            let case = format!("{case} against {store}");
            let refused = expect_error(
                &case,
                call(router, "CancelExportTask", &[("ExportTaskIdentifier", identifier)]).await,
                404,
                "ExportTaskNotFound",
            );
            let refused = text(&refused, "Message");
            assert_eq!(
                refused,
                export_task_sentence(identifier),
                "case {case:?}: expected the sentence naming the requested identifier, got \
                 {refused:?}"
            );
        }
    }

    // The task the stocked store DOES hold is still cancelled, so the refusals above are
    // about resolving the identifier rather than about the operation.
    let canceled = expect_ok(
        "the task the store holds",
        call(
            &stocked,
            "CancelExportTask",
            &[("ExportTaskIdentifier", "exp-in-progress")],
        )
        .await,
    );
    assert_eq!(
        text(&canceled, "Status"),
        "CANCELING",
        "expected the cancellation of a stored task to be served, got {canceled}"
    );
}

#[tokio::test]
async fn the_blue_green_paths_name_the_deployment_identifier_they_resolved() {
    let router = emulator();

    // A deployment this emulator MINTED and then removed, so one row's variable part is a
    // value this test never spells out.
    create_cluster(&router, REGION, "emu-sentence-bg-cluster").await;
    wait_for_settle();
    let created = expect_ok(
        "the deployment that is deleted below",
        call(
            &router,
            "CreateBlueGreenDeployment",
            &[
                ("BlueGreenDeploymentName", "emu-sentence-bg"),
                (
                    "Source",
                    &format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-sentence-bg-cluster"),
                ),
            ],
        )
        .await,
    );
    let minted = text(&created, "BlueGreenDeploymentIdentifier");
    assert!(
        !minted.is_empty(),
        "the create must report the identifier it minted, got {created}"
    );
    expect_ok(
        "the delete of a deployment that exists",
        call(
            &router,
            "DeleteBlueGreenDeployment",
            &[("BlueGreenDeploymentIdentifier", minted.as_str())],
        )
        .await,
    );
    wait_for_settle();

    // `(case, the value the request carries, the identifier the sentence must quote)`. The
    // member accepts the deployment's ARN as well as its identifier, so the quoted part is
    // the identifier either form carries — in the spelling the operation RESOLVED it in,
    // which is the request's own for the delete and the folded one for the report whose
    // member the model documents as not case-sensitive (see
    // `source_lookup_and_filter_refusal_sentences`).
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an identifier of the assigned form that names nothing",
            "bgd-0123456789abcdef".to_string(),
            "bgd-0123456789abcdef".to_string(),
        ),
        (
            "an identifier whose assigned token is in another case",
            "bgd-0123456789ABCDEF".to_string(),
            "bgd-0123456789ABCDEF".to_string(),
        ),
        (
            "an ARN carrying an identifier that names nothing",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:blue-green-deployment:bgd-00000000deadbeef"),
            "bgd-00000000deadbeef".to_string(),
        ),
        (
            "another identifier of the assigned form that names nothing",
            "bgd-fedcba9876543210".to_string(),
            "bgd-fedcba9876543210".to_string(),
        ),
        (
            "the identifier of a deployment whose delete has run",
            minted.clone(),
            minted.clone(),
        ),
    ];

    for (case, named, quoted) in &rows {
        for action in ["DeleteBlueGreenDeployment", "DescribeBlueGreenDeployments"] {
            let refused = expect_error(
                case,
                call(
                    &router,
                    action,
                    &[("BlueGreenDeploymentIdentifier", named.as_str())],
                )
                .await,
                404,
                "BlueGreenDeploymentNotFoundFault",
            );
            let refused = text(&refused, "Message");
            let quoted = if action == "DescribeBlueGreenDeployments" {
                quoted.to_ascii_lowercase()
            } else {
                quoted.clone()
            };
            assert_eq!(
                refused,
                blue_green_sentence(&quoted),
                "case {case:?}: expected {action} to quote the deployment identifier, got \
                 {refused:?}"
            );
        }
    }
}
