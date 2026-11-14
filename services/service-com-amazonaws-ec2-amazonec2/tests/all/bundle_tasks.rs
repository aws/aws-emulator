// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeBundleTasks`, driven over the wire the service actually
//! speaks — `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeBundleTasks`, answered with XML — through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the report is THE STORE: every member a stored bundle task carries reaches the wire as
//!    stored, the nested storage location and the task error included, and a call that
//!    narrows nothing reports every bundle task the account holds — an account holding none
//!    is reported as holding none rather than refused ("describes the specified bundle tasks
//!    or all of your bundle tasks");
//! 2. `BundleIds` NARROWS the report, and any id of it this account holds no bundle task for
//!    — a well-formed id no task was minted under, or a value that is not a bundle task id at
//!    all — is REFUSED with EC2's own `InvalidBundleID.NotFound`, whose sentence names every
//!    id at fault, so an id that resolves to nothing is never answered as an empty report;
//! 3. `Filters` select with EC2's AND/OR rule — "If you specify multiple filters, the filters
//!    are joined with an AND" and, within one filter, the values with an OR (`Filter`,
//!    `Filter$Values`) — over every name this operation's `Filters` member publishes, with
//!    EC2's `*`/`?` wildcards, and a name it does not publish is REFUSED rather than silently
//!    selecting nothing;
//! 4. every request-controlled factor of the match is BOUNDED: a request AT a published bound
//!    is answered and one past it is refused with `InvalidParameterValue`;
//! 5. `DryRun` reports the permission answer and reports no task, and a dry run of a request
//!    that would have been refused is refused with that reason instead;
//! 6. the report is a READ: repeated identical calls answer the same set and leave the store
//!    exactly as it was.
//!
//! Each table row is a distinct input along what the rule generalises over — which state,
//! progress, storage location and error a task carries, which ids narrow the report, which
//! filter name, how many values, which bound and which side of it — so no row is the
//! specification. Between them the rows cover every value of the published `BundleTaskState`
//! catalog, which [`the_rows_cover_every_published_bundle_task_state`] pins.
//!
//! Bundle tasks are SEEDED into the store here, the way the sibling export-task suite
//! ([`super::export_tasks`]) seeds export tasks: `BundleInstance`, the operation that started
//! one, and `CancelBundleTask`, the one that cancelled it, are both actions EC2 no longer
//! serves ([`super::retired_bundle_api_actions`]), so the store is the only writer available
//! — and seeding is what lets one row per published catalog value be a case.

use ec2::ops::types::{
    BUNDLE_TASK_STATE_VALUES, BundleTask, BundleTaskError, S3Storage, Storage,
};
use ec2::state::{
    BUNDLE_TASK_ID_PREFIX, ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION,
    INVALID_BUNDLE_ID_NOT_FOUND, INVALID_PARAMETER_VALUE, MAX_FILTER_VALUE_BYTES,
    MAX_FILTER_VALUES, MAX_FILTERS, MAX_RESOURCE_IDS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, filters, indexed};
use super::wire::{child, child_text, document, param};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because it was malformed.
const DRY_RUN_STATUS: u16 = 412;

/// The instant the earliest seeded task started, as seconds since the epoch. Every other
/// seeded instant is an offset from it, so the timestamp filters select on a value the table
/// fixes rather than on a clock reading.
const BASE_SECS: i64 = 1_767_225_600;

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeBundleTasks", params).await
}

/// The `BundleId.N` form fields for a narrowing request: the member's `@xmlName`
/// (`smithy_query(query="operation",operation="DescribeBundleTasks")` gives `BundleId`).
fn ids(values: &[&str]) -> Params {
    indexed("BundleId", values)
}

/// A timestamp as both the store holds it and the wire carries it.
fn at(secs: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs(secs)
}

/// `secs` as the text a timestamp member is written as on the wire, which is also what a
/// client sends a timestamp filter's value as.
fn at_text(secs: i64) -> String {
    at(secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("a representable timestamp")
}

/// One bundle task a row names: what varies between the rows of the table.
///
/// `error` is absent on a task that has not failed ("if the task fails, a description of the
/// error" — `BundleTask$BundleTaskError`), and `s3_prefix` is absent on a task stored under
/// no AMI name prefix ("the beginning of the file name of the AMI" — `S3Storage$Prefix`).
struct TaskSpec<'a> {
    instance_id: &'a str,
    state: &'a str,
    progress: &'a str,
    s3_bucket: &'a str,
    s3_prefix: Option<&'a str>,
    start_secs: i64,
    update_secs: i64,
    error: Option<(&'a str, &'a str)>,
}

/// `spec` as the modelled shape the describe must report.
fn bundle_task(spec: TaskSpec<'_>) -> BundleTask {
    let TaskSpec {
        instance_id,
        state,
        progress,
        s3_bucket,
        s3_prefix,
        start_secs,
        update_secs,
        error,
    } = spec;
    BundleTask {
        instance_id: Some(instance_id.to_string()),
        bundle_id: None,
        state: Some(state.to_string()),
        start_time: Some(at(start_secs)),
        update_time: Some(at(update_secs)),
        storage: Some(Storage {
            s3: Some(S3Storage {
                aws_access_key_id: None,
                bucket: Some(s3_bucket.to_string()),
                prefix: s3_prefix.map(str::to_string),
                upload_policy: None,
                upload_policy_signature: None,
            }),
        }),
        progress: Some(progress.to_string()),
        bundle_task_error: error.map(|(code, message)| BundleTaskError {
            code: Some(code.to_string()),
            message: Some(message.to_string()),
        }),
    }
}

/// Store `view` under a freshly minted bundle task id, and answer that id.
fn seed(engine: &engine::InMemoryEngine, view: &BundleTask) -> String {
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    let bundle_id = store.next_id(BUNDLE_TASK_ID_PREFIX);
    let mut view = view.clone();
    view.bundle_id = Some(bundle_id.clone());
    store.bundle_tasks.insert(bundle_id.clone(), view);
    bundle_id
}

/// How many bundle tasks the store holds.
fn stored_tasks(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().bundle_tasks.len()
}

/// One bundle task of this suite's table, seeded into `engine`'s store, answered as
/// `(its id, the task as stored)`.
///
/// For the sibling suite that drives the two bundle API actions EC2 no longer serves
/// ([`super::retired_bundle_api_actions`]): it needs a bundle id that RESOLVES, and the store
/// is the only writer of a bundle task — so the seeding lives here, with the table it reads
/// its row from, rather than being written a second time there.
pub(super) fn seed_one_task(engine: &engine::InMemoryEngine) -> (String, BundleTask) {
    let (_case, view) = rows()
        .into_iter()
        .next()
        .expect("the table must hold at least one bundle task");
    let bundle_id = seed(engine, &view);
    let mut stored = view;
    stored.bundle_id = Some(bundle_id.clone());
    (bundle_id, stored)
}

/// The table rules 1, 2, 3 and 6 work from: one bundle task per row, each row a distinct
/// combination of what a bundle task can carry — including one row per value of the published
/// `BundleTaskState` catalog.
fn rows() -> Vec<(&'static str, BundleTask)> {
    vec![
        (
            "a task that has only just been accepted",
            bundle_task(TaskSpec {
                instance_id: "i-0123456789abcdef0",
                state: "pending",
                progress: "0%",
                s3_bucket: "my-bundles",
                s3_prefix: Some("winsrv-a"),
                start_secs: BASE_SECS,
                update_secs: BASE_SECS,
                error: None,
            }),
        ),
        (
            "a task waiting for the instance to shut down, with no AMI name prefix",
            bundle_task(TaskSpec {
                instance_id: "i-00000000000000001",
                state: "waiting-for-shutdown",
                progress: "0%",
                s3_bucket: "my-bundles",
                s3_prefix: None,
                start_secs: BASE_SECS + 60,
                update_secs: BASE_SECS + 120,
                error: None,
            }),
        ),
        (
            "a task part way through bundling the root volume",
            bundle_task(TaskSpec {
                instance_id: "i-00000000000000002",
                state: "bundling",
                progress: "20%",
                s3_bucket: "nightly-bundles",
                s3_prefix: Some("winsrv-b"),
                start_secs: BASE_SECS + 180,
                update_secs: BASE_SECS + 600,
                error: None,
            }),
        ),
        (
            "a task uploading the bundle into its bucket",
            bundle_task(TaskSpec {
                instance_id: "i-00000000000000003",
                state: "storing",
                progress: "80%",
                s3_bucket: "nightly-bundles",
                s3_prefix: Some("winsrv-c"),
                start_secs: BASE_SECS + 240,
                update_secs: BASE_SECS + 900,
                error: None,
            }),
        ),
        (
            "a task a cancel is taking down",
            bundle_task(TaskSpec {
                instance_id: "i-00000000000000004",
                state: "cancelling",
                progress: "35%",
                s3_bucket: "my-bundles",
                s3_prefix: Some("winsrv-d"),
                start_secs: BASE_SECS + 300,
                update_secs: BASE_SECS + 960,
                error: None,
            }),
        ),
        (
            "a finished task",
            bundle_task(TaskSpec {
                instance_id: "i-00000000000000005",
                state: "complete",
                progress: "100%",
                s3_bucket: "archive-bundles",
                s3_prefix: Some("winsrv-e"),
                start_secs: BASE_SECS + 360,
                update_secs: BASE_SECS + 3_600,
                error: None,
            }),
        ),
        (
            "a failed task, which is the only kind that carries an error",
            bundle_task(TaskSpec {
                instance_id: "i-00000000000000006",
                state: "failed",
                progress: "45%",
                s3_bucket: "archive-bundles",
                s3_prefix: Some("winsrv-f"),
                start_secs: BASE_SECS + 420,
                update_secs: BASE_SECS + 1_800,
                error: Some(("InternalError", "the bundle could not be uploaded")),
            }),
        ),
    ]
}

/// Seed [`rows`] and answer `(case, the stored task, its id)` per row.
fn seed_rows(engine: &engine::InMemoryEngine) -> Vec<(&'static str, BundleTask, String)> {
    rows()
        .into_iter()
        .map(|(case, view)| {
            let bundle_id = seed(engine, &view);
            let mut stored = view;
            stored.bundle_id = Some(bundle_id.clone());
            (case, stored, bundle_id)
        })
        .collect()
}

/// One bundle task as a response reported it: every member flattened to the text on the wire,
/// so what the describe wrote can be compared against what the store holds.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    bundle_id: String,
    instance_id: String,
    state: String,
    progress: String,
    s3_bucket: String,
    s3_prefix: String,
    start_time: String,
    update_time: String,
    error_code: String,
    error_message: String,
}

/// A stored bundle task projected the way [`reported`] reads one off the wire, so the two can
/// be compared member by member. An absent member is the empty string on both sides.
fn expected(task: &BundleTask) -> Reported {
    let s3 = task
        .storage
        .clone()
        .unwrap_or_default()
        .s3
        .unwrap_or_default();
    let error = task.bundle_task_error.clone().unwrap_or_default();
    let text = |value: Option<String>| value.unwrap_or_default();
    let instant = |value: Option<::aws_smithy_types::DateTime>| {
        value
            .and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok())
            .unwrap_or_default()
    };
    Reported {
        bundle_id: text(task.bundle_id.clone()),
        instance_id: text(task.instance_id.clone()),
        state: text(task.state.clone()),
        progress: text(task.progress.clone()),
        s3_bucket: text(s3.bucket),
        s3_prefix: text(s3.prefix),
        start_time: instant(task.start_time),
        update_time: instant(task.update_time),
        error_code: text(error.code),
        error_message: text(error.message),
    }
}

/// Every bundle task a response carries, in the order it reported them.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "bundleId").is_some())
        .map(|item| {
            // `BundleTask$Storage` is written as `<storage>` and `Storage$S3` as `<S3>`
            // (`smithy_query(query="shape",shape="Storage")`), so the bucket and the prefix
            // are read through both.
            let s3 = |name: &str| {
                child(item, "storage")
                    .and_then(|storage| child(storage, "S3"))
                    .and_then(|s3| child_text(s3, name))
                    .unwrap_or_default()
            };
            // `BundleTask$BundleTaskError` is written as `<error>`.
            let error = |name: &str| {
                child(item, "error")
                    .and_then(|error| child_text(error, name))
                    .unwrap_or_default()
            };
            Reported {
                bundle_id: child_text(item, "bundleId").unwrap_or_default(),
                instance_id: child_text(item, "instanceId").unwrap_or_default(),
                state: child_text(item, "state").unwrap_or_default(),
                progress: child_text(item, "progress").unwrap_or_default(),
                s3_bucket: s3("bucket"),
                s3_prefix: s3("prefix"),
                start_time: child_text(item, "startTime").unwrap_or_default(),
                update_time: child_text(item, "updateTime").unwrap_or_default(),
                error_code: error("code"),
                error_message: error("message"),
            }
        })
        .collect()
}

/// Every `bundleId` a response reported, sorted, so a selection can be compared whatever
/// order the report took ("the order of the elements in the response ... might vary").
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    let mut found: Vec<String> = reported(case, body)
        .into_iter()
        .map(|task| task.bundle_id)
        .collect();
    found.sort();
    found
}

/// The ids `params` selects, out of a call that must succeed.
async fn selected(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    reported_ids(case, &body)
}

// ---------------------------------------------------------------------------
// The table is tied to the model's own catalog
// ---------------------------------------------------------------------------

/// Every value of the published `BundleTaskState` catalog appears in [`rows`], so a value the
/// model adds cannot leave the table narrower than the catalog unnoticed.
#[test]
fn the_rows_cover_every_published_bundle_task_state() {
    let seeded = rows();
    let states: Vec<String> = seeded
        .iter()
        .filter_map(|(_, task)| task.state.clone())
        .collect();
    for published in BUNDLE_TASK_STATE_VALUES {
        assert!(
            states.iter().any(|state| state == published),
            "the table covers no task in the published state {published:?}, only {states:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 1: the report is the store
// ---------------------------------------------------------------------------

/// A call that narrows nothing reports every stored bundle task, each member as stored.
#[tokio::test]
async fn an_unnarrowed_call_reports_every_stored_task_as_stored() {
    let (router, engine) = emulator();

    // An account that holds no bundle task is reported as holding none, rather than refused:
    // "describes the specified bundle tasks or all of your bundle tasks".
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "an empty store: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        reported("an empty store", &body).is_empty(),
        "an empty store: expected no bundle task, got {body}"
    );

    let seeded = seed_rows(&engine);
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the seeded store: expected HTTP 200, got {status} with body {body}"
    );
    let found = reported("the seeded store", &body);
    assert_eq!(
        found.len(),
        seeded.len(),
        "the seeded store: expected {} tasks, got {found:?}",
        seeded.len()
    );
    for (case, stored, bundle_id) in &seeded {
        let wanted = expected(stored);
        let got = found
            .iter()
            .find(|task| &task.bundle_id == bundle_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: {bundle_id} is not in the report {found:?}")
            });
        assert_eq!(
            got, &wanted,
            "case {case:?}: expected the task as stored {wanted:?}, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: BundleIds narrows, and an id that resolves to nothing is refused
// ---------------------------------------------------------------------------

/// `BundleIds` narrows the report to the ids named, and an id this account holds no bundle
/// task for is REFUSED — whatever shape that id has — rather than reported as an empty
/// selection.
#[tokio::test]
async fn bundle_ids_narrow_the_report_and_an_unheld_id_is_refused() {
    let (router, engine) = emulator();
    let seeded = seed_rows(&engine);
    let held: Vec<String> = seeded
        .iter()
        .map(|(_, _, bundle_id)| bundle_id.clone())
        .collect();
    let sorted = |mut values: Vec<String>| {
        values.sort();
        values
    };

    // `(case, the ids the request names, the ids it must select)`.
    let id_rows: Vec<(String, Params, Vec<String>)> = vec![
        (
            "one id the account holds".to_string(),
            ids(&[held[0].as_str()]),
            vec![held[0].clone()],
        ),
        (
            "two ids the account holds".to_string(),
            ids(&[held[1].as_str(), held[4].as_str()]),
            sorted(vec![held[1].clone(), held[4].clone()]),
        ),
        (
            "the same id twice".to_string(),
            ids(&[held[2].as_str(), held[2].as_str()]),
            vec![held[2].clone()],
        ),
        (
            "every id the account holds".to_string(),
            ids(&held.iter().map(String::as_str).collect::<Vec<&str>>()),
            sorted(held.clone()),
        ),
        (
            "no id at all, which narrows nothing".to_string(),
            Vec::new(),
            sorted(held.clone()),
        ),
    ];
    for (case, params, wanted) in id_rows {
        let got = selected(&router, &case, &params).await;
        assert_eq!(
            got, wanted,
            "case {case:?}: expected the tasks {wanted:?}, got {got:?}"
        );
    }

    // `(case, the ids the request names, the sentence it is refused with)`: a well-formed
    // long-form id and a well-formed short-form id the account holds no task under, values
    // that are not bundle task ids at all, and two unresolvable ids at once — EC2's refusal
    // naming EVERY id at fault, under the one code it publishes for the member.
    let mut unresolvable: Vec<(String, Vec<String>, String)> = Vec::new();
    for value in [
        format!("{BUNDLE_TASK_ID_PREFIX}-0123456789abcdef0"),
        format!("{BUNDLE_TASK_ID_PREFIX}-00000000"),
        "not-a-bundle-task".to_string(),
        "i-0123456789abcdef0".to_string(),
        "bun-not-hex-at-all".to_string(),
        "BUN-0123456789ABCDEF0".to_string(),
        "bun-".to_string(),
    ] {
        unresolvable.push((
            format!("the id {value:?}, which this account holds no bundle task under"),
            vec![value.clone()],
            format!("Invalid bundle ids: {value}"),
        ));
    }
    let first = format!("{BUNDLE_TASK_ID_PREFIX}-0123456789abcdef0");
    let second = format!("{BUNDLE_TASK_ID_PREFIX}-fedcba9876543210f");
    unresolvable.push((
        "two ids that resolve to nothing, both named in the refusal".to_string(),
        vec![first.clone(), second.clone()],
        format!("Invalid bundle ids: {first}, {second}"),
    ));
    unresolvable.push((
        "the same unresolvable id twice, named once in the refusal".to_string(),
        vec![first.clone(), first.clone()],
        format!("Invalid bundle ids: {first}"),
    ));

    for (case, values, sentence) in unresolvable {
        let named: Vec<&str> = values.iter().map(String::as_str).collect();
        // On their own, and beside an id the account DOES hold: an id that resolves to
        // nothing is refused either way, so a held id can never mask one.
        let mut beside = vec![held[0].as_str()];
        beside.extend(named.iter().copied());
        for (where_, params) in [
            ("on its own", ids(&named)),
            ("beside an id the account holds", ids(&beside)),
        ] {
            let case = format!("{case} {where_}");
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_BUNDLE_ID_NOT_FOUND,
                "case {case:?}: expected {INVALID_BUNDLE_ID_NOT_FOUND}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                sentence,
                "case {case:?}: expected the message {sentence:?}, got {body}"
            );
            assert!(
                reported(&case, &body).is_empty(),
                "case {case:?}: a refused request reports no bundle task, got {body}"
            );
        }
    }

    // None of those refusals touched the store.
    assert_eq!(
        stored_tasks(&engine),
        seeded.len(),
        "the refusals left the store holding {} tasks",
        stored_tasks(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 3: filtering
// ---------------------------------------------------------------------------

/// Each filter name this operation publishes selects on the member it names, several values
/// of one filter are joined with an OR, several filters with an AND, and EC2's `*`/`?`
/// wildcards match within a value.
#[tokio::test]
async fn filters_select_with_ec2s_and_or_rule_over_every_published_name() {
    let (router, engine) = emulator();
    let seeded = seed_rows(&engine);
    let id_of = |index: usize| seeded[index].2.clone();
    let sorted = |mut values: Vec<String>| {
        values.sort();
        values
    };

    // `(case, the filter parameters, the ids they must select)`. Every name
    // `DescribeBundleTasksRequest$Filters` publishes appears at least once.
    let filter_rows: Vec<(String, Params, Vec<String>)> = vec![
        (
            "bundle-id names one task".to_string(),
            filters(&[("bundle-id", &[seeded[1].2.as_str()])]),
            vec![id_of(1)],
        ),
        (
            "bundle-id with two values is an OR".to_string(),
            filters(&[("bundle-id", &[seeded[0].2.as_str(), seeded[3].2.as_str()])]),
            sorted(vec![id_of(0), id_of(3)]),
        ),
        (
            "bundle-id naming no stored task selects nothing".to_string(),
            filters(&[("bundle-id", &["bun-0123456789abcdef0"])]),
            Vec::new(),
        ),
        (
            "instance-id selects the task bundling that instance".to_string(),
            filters(&[("instance-id", &["i-00000000000000002"])]),
            vec![id_of(2)],
        ),
        (
            "state selects the task in that state".to_string(),
            filters(&[("state", &["complete"])]),
            vec![id_of(5)],
        ),
        (
            "state with three values is an OR".to_string(),
            filters(&[("state", &["pending", "bundling", "storing"])]),
            sorted(vec![id_of(0), id_of(2), id_of(3)]),
        ),
        (
            "state in the wrong case selects nothing, filter values being \
             case-sensitive"
                .to_string(),
            filters(&[("state", &["COMPLETE"])]),
            Vec::new(),
        ),
        (
            "progress matches the whole value".to_string(),
            filters(&[("progress", &["20%"])]),
            vec![id_of(2)],
        ),
        (
            "a ? wildcard matches exactly one character of progress, so a two-character \
             value is not selected"
                .to_string(),
            filters(&[("progress", &["?0%"])]),
            sorted(vec![id_of(2), id_of(3)]),
        ),
        (
            "s3-bucket selects every task stored in that bucket".to_string(),
            filters(&[("s3-bucket", &["nightly-bundles"])]),
            sorted(vec![id_of(2), id_of(3)]),
        ),
        (
            "a * wildcard matches within s3-bucket, and every seeded bucket ends the same \
             way"
                .to_string(),
            filters(&[("s3-bucket", &["*-bundles"])]),
            sorted(vec![
                id_of(0),
                id_of(1),
                id_of(2),
                id_of(3),
                id_of(4),
                id_of(5),
                id_of(6),
            ]),
        ),
        (
            "a * wildcard at the end of s3-bucket selects only that bucket's tasks".to_string(),
            filters(&[("s3-bucket", &["archive-*"])]),
            sorted(vec![id_of(5), id_of(6)]),
        ),
        (
            "s3-prefix selects the task stored under that AMI name".to_string(),
            filters(&[("s3-prefix", &["winsrv-c"])]),
            vec![id_of(3)],
        ),
        (
            "a * wildcard on s3-prefix selects every task that carries one, and no task \
             that carries none"
                .to_string(),
            filters(&[("s3-prefix", &["*"])]),
            sorted(vec![
                id_of(0),
                id_of(2),
                id_of(3),
                id_of(4),
                id_of(5),
                id_of(6),
            ]),
        ),
        (
            "error-code selects the failed task".to_string(),
            filters(&[("error-code", &["InternalError"])]),
            vec![id_of(6)],
        ),
        (
            "a * wildcard on error-code selects only a task that failed".to_string(),
            filters(&[("error-code", &["*"])]),
            vec![id_of(6)],
        ),
        (
            "error-message selects on the text of the error".to_string(),
            filters(&[("error-message", &["*could not be uploaded"])]),
            vec![id_of(6)],
        ),
        (
            "start-time selects on the instant the task started".to_string(),
            filters(&[("start-time", &[at_text(BASE_SECS + 180).as_str()])]),
            vec![id_of(2)],
        ),
        (
            "start-time naming an instant no task started at selects nothing".to_string(),
            filters(&[("start-time", &[at_text(BASE_SECS - 1).as_str()])]),
            Vec::new(),
        ),
        (
            "update-time selects on the most recent update".to_string(),
            filters(&[("update-time", &[at_text(BASE_SECS + 900).as_str()])]),
            vec![id_of(3)],
        ),
        (
            "two filters are joined with an AND".to_string(),
            filters(&[("s3-bucket", &["nightly-bundles"]), ("state", &["storing"])]),
            vec![id_of(3)],
        ),
        (
            "an AND no task satisfies selects nothing".to_string(),
            filters(&[("s3-bucket", &["nightly-bundles"]), ("state", &["failed"])]),
            Vec::new(),
        ),
        (
            "a filter beside BundleIds narrows further".to_string(),
            {
                let mut params = ids(&[seeded[2].2.as_str(), seeded[3].2.as_str()]);
                params.extend(filters(&[("state", &["storing"])]));
                params
            },
            vec![id_of(3)],
        ),
    ];

    // Every published name is exercised by the rows above, so a name this operation answers
    // can never go untested.
    for published in ec2_published_filter_names() {
        assert!(
            filter_rows.iter().any(|(_, params, _)| params
                .iter()
                .any(|(key, value)| key.ends_with(".Name") && value == published)),
            "no row filters on the published name {published:?}"
        );
    }

    for (case, params, wanted) in filter_rows {
        let got = selected(&router, &case, &params).await;
        assert_eq!(
            got, wanted,
            "case {case:?}: expected the tasks {wanted:?}, got {got:?}"
        );
    }
}

/// The filter names `DescribeBundleTasksRequest$Filters` publishes:
///
/// ```text
/// smithy_query(query="doc",shape="DescribeBundleTasksRequest",member="Filters",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
fn ec2_published_filter_names() -> [&'static str; 10] {
    [
        "bundle-id",
        "error-code",
        "error-message",
        "instance-id",
        "progress",
        "s3-bucket",
        "s3-prefix",
        "start-time",
        "state",
        "update-time",
    ]
}

/// A filter name this operation does not publish is REFUSED, so a misspelling can never read
/// as "no bundle tasks".
#[tokio::test]
async fn a_filter_name_this_operation_does_not_publish_is_refused() {
    let (router, engine) = emulator();
    seed_rows(&engine);

    // `(case, the filter name)`: a name of no resemblance, a published name in the wrong case
    // (EC2 filter names are case-sensitive), a published name misspelled, a tag filter —
    // which a bundle task, carrying no `Tags` member, has nothing to answer with — and the
    // empty name.
    let name_rows: [(&str, &str); 6] = [
        ("a name of no resemblance", "sim2real-not-a-real-filter"),
        ("a published name in the wrong case", "State"),
        ("a published name misspelled", "bundle-ids"),
        ("the tag-key filter of a taggable resource", "tag-key"),
        ("a tag filter naming a key", "tag:Name"),
        ("the empty name", ""),
    ];
    for (case, name) in name_rows {
        let (status, body) = describe(&router, &filters(&[(name, &["any"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 4: every request-controlled factor is bounded
// ---------------------------------------------------------------------------

/// A request AT each published bound is answered, and one PAST it is refused with
/// `InvalidParameterValue` — the ids it names, the filters it carries, the values of one
/// filter, and the size of one value.
#[tokio::test]
async fn each_published_bound_is_answered_and_one_past_it_is_refused() {
    let (router, engine) = emulator();
    let seeded = seed_rows(&engine);
    let held = seeded[0].2.clone();

    let id_list = |count: usize| -> Params {
        // Every entry names a task the account HOLDS, cycling through the seeded ids, so the
        // row is about the NUMBER of ids rather than about whether one of them resolves.
        (0..count)
            .map(|index| {
                (
                    format!("BundleId.{}", index + 1),
                    seeded[index % seeded.len()].2.clone(),
                )
            })
            .collect()
    };
    // Each filter carries a published name, cycling through them, so the row is about the
    // NUMBER of filters rather than about any one name.
    let filter_list = |count: usize| -> Params {
        let names = ec2_published_filter_names();
        (0..count)
            .flat_map(|index| {
                vec![
                    (
                        format!("Filter.{}.Name", index + 1),
                        names[index % names.len()].to_string(),
                    ),
                    (format!("Filter.{}.Value.1", index + 1), "*".to_string()),
                ]
            })
            .collect()
    };
    let value_list = |count: usize| -> Params {
        let mut params = vec![("Filter.1.Name".to_string(), "state".to_string())];
        params.extend((0..count).map(|index| {
            (
                format!("Filter.1.Value.{}", index + 1),
                format!("state-{index}"),
            )
        }));
        params
    };
    let value_of_bytes =
        |bytes: usize| -> Params { filters(&[("progress", &["x".repeat(bytes).as_str()])]) };

    // `(case, the parameters, whether the request is answered)`.
    let bound_rows: Vec<(String, Params, bool)> = vec![
        (
            format!("{MAX_RESOURCE_IDS} ids, the bound itself"),
            id_list(MAX_RESOURCE_IDS),
            true,
        ),
        (
            format!("{} ids, one past the bound", MAX_RESOURCE_IDS + 1),
            id_list(MAX_RESOURCE_IDS + 1),
            false,
        ),
        (
            format!("{MAX_FILTERS} filters, the bound itself"),
            filter_list(MAX_FILTERS),
            true,
        ),
        (
            format!("{} filters, one past the bound", MAX_FILTERS + 1),
            filter_list(MAX_FILTERS + 1),
            false,
        ),
        (
            format!("{MAX_FILTER_VALUES} values of one filter, the bound itself"),
            value_list(MAX_FILTER_VALUES),
            true,
        ),
        (
            format!(
                "{} values of one filter, one past the bound",
                MAX_FILTER_VALUES + 1
            ),
            value_list(MAX_FILTER_VALUES + 1),
            false,
        ),
        (
            format!("a {MAX_FILTER_VALUE_BYTES}-byte filter value, the bound itself"),
            value_of_bytes(MAX_FILTER_VALUE_BYTES),
            true,
        ),
        (
            format!(
                "a {}-byte filter value, one past the bound",
                MAX_FILTER_VALUE_BYTES + 1
            ),
            value_of_bytes(MAX_FILTER_VALUE_BYTES + 1),
            false,
        ),
    ];

    for (case, params, answered) in bound_rows {
        let (status, body) = describe(&router, &params).await;
        if answered {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
            );
        }
    }

    // Nothing any of those requests did removed a task or added one.
    assert_eq!(
        stored_tasks(&engine),
        seeded.len(),
        "the bound rows left the store holding {} tasks",
        stored_tasks(&engine)
    );
    let got = selected(&router, "after the bound rows", &ids(&[held.as_str()])).await;
    assert_eq!(
        got,
        vec![held.clone()],
        "after the bound rows the held task is still reported, got {got:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 5: a dry run answers the permission question and reports nothing
// ---------------------------------------------------------------------------

/// `DryRun` reports the permission answer instead of the tasks — whether or not the ids it
/// names resolve, the flag asking "would this call be permitted" and not "does this task
/// exist" — and a dry run of a request that would have been refused anyway is refused with
/// THAT reason.
#[tokio::test]
async fn a_dry_run_reports_the_permission_answer_and_reports_no_task() {
    let (router, engine) = emulator();
    let seeded = seed_rows(&engine);
    let held = seeded[0].2.clone();

    // `(case, the parameters beside DryRun, the status, the error code)`: a dry run of a
    // request that would have succeeded, of one narrowed to a held id, of one naming an id
    // the account holds no task for — still the permission answer — and of one carrying a
    // filter name this operation does not publish, which is refused for THAT reason.
    let dry_rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "a dry run that narrows nothing",
            Vec::new(),
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
        ),
        (
            "a dry run narrowed to a held id",
            ids(&[held.as_str()]),
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
        ),
        (
            "a dry run naming an id the account holds no task for",
            ids(&[format!("{BUNDLE_TASK_ID_PREFIX}-0123456789abcdef0").as_str()]),
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a request that would have been refused",
            filters(&[("sim2real-not-a-real-filter", &["any"])]),
            400,
            INVALID_PARAMETER_VALUE,
        ),
    ];

    for (case, extra, wanted_status, wanted_code) in dry_rows {
        let mut params = vec![param("DryRun", "true")];
        params.extend(extra);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, wanted_status,
            "case {case:?}: expected HTTP {wanted_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            wanted_code,
            "case {case:?}: expected {wanted_code}, got {body}"
        );
        assert!(
            reported(case, &body).is_empty(),
            "case {case:?}: a refused request reports no bundle task, got {body}"
        );
    }

    // `DryRun=false` is not a dry run at all, so it is served.
    let got = selected(
        &router,
        "DryRun=false",
        &[param("DryRun", "false"), ("BundleId.1".to_string(), held.clone())],
    )
    .await;
    assert_eq!(
        got,
        vec![held.clone()],
        "DryRun=false is served: expected the held task, got {got:?}"
    );
    assert_eq!(
        stored_tasks(&engine),
        seeded.len(),
        "the dry runs left the store holding {} tasks",
        stored_tasks(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 6: the report is a read
// ---------------------------------------------------------------------------

/// Describing bundle tasks changes nothing a later caller sees: repeated identical calls
/// report the same set and the store holds the same tasks, member for member.
#[tokio::test]
async fn describing_bundle_tasks_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    let seeded = seed_rows(&engine);

    let (_, first) = describe(&router, &[]).await;
    let before = reported("the first call", &first);
    for _ in 0..3 {
        let (status, body) = describe(&router, &[]).await;
        assert_eq!(
            status, 200,
            "a repeated call: expected HTTP 200, got {status} with body {body}"
        );
        let again = reported("a repeated call", &body);
        assert_eq!(
            again, before,
            "a repeated call must report the same set: expected {before:?}, got {again:?}"
        );
    }
    // A filtered and a narrowed read are reads too.
    let _ = selected(
        &router,
        "a filtered read",
        &filters(&[("state", &["bundling"])]),
    )
    .await;
    let _ = selected(
        &router,
        "a narrowed read",
        &ids(&[seeded[0].2.as_str()]),
    )
    .await;

    assert_eq!(
        stored_tasks(&engine),
        seeded.len(),
        "the reads left the store holding {} tasks",
        stored_tasks(&engine)
    );
    for (case, stored, bundle_id) in &seeded {
        let still = engine
            .state::<Ec2State>()
            .lock()
            .bundle_tasks
            .get(bundle_id)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: {bundle_id} is no longer stored"));
        assert_eq!(
            expected(&still),
            expected(stored),
            "case {case:?}: the reads changed the stored task, got {:?}",
            expected(&still)
        );
    }
}
